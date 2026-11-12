//! An edit that imports external audio decodes it on its own thread.
//!
//! Decoding and writing a long file takes hundreds of milliseconds. Staging
//! changes no history, so it runs beside the worker. Meanwhile the worker
//! keeps transport, device requests, the playhead and meters moving, and holds
//! later edits, history requests, Undo and Redo behind the import in order.
//! Bytes captured or saved by plugins wait too. So the edit applies to the
//! document as it was when submitted, and Undo after it undoes it (ADR 0331).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use echode_actions::Command;
use echode_history::{
    DocumentHost, StagedImports, command_import_source, transaction_import_sources,
};
use serde_json::Value;

use crate::{DocumentError, Operation, Rejected};

/// Stages an edit's sources into the package, stopping once the flag says so.
/// The product's is [`StagedImports::stage`]; tests hold it to watch the turns
/// taken meanwhile.
pub(crate) type Stage =
    Arc<dyn Fn(&Path, &[String], &dyn Fn() -> bool) -> StagedImports + Send + Sync>;

pub(crate) fn product_stage() -> Stage {
    Arc::new(StagedImports::stage)
}

/// Whether `operation` must wait for an import submitted before it: anything
/// that reads or changes the history in submission order.
pub(crate) fn follows_imports(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Perform(_) | Operation::History(_) | Operation::Undo | Operation::Redo
    )
}

/// One command, or an array of actions as a single Undo step.
pub(crate) enum Edit {
    Command(Command),
    Group(Vec<Value>),
}

impl Edit {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, DocumentError> {
        let text = match str::from_utf8(bytes) {
            Ok(text) if !text.is_empty() => text,
            Ok(_) => return Err(Rejected::EmptyCommand.into()),
            Err(_) => return Err(Rejected::NotUtf8.into()),
        };
        if text.trim_start().starts_with('[') {
            let actions = serde_json::from_str(text).map_err(DocumentError::Malformed)?;
            return Ok(Self::Group(actions));
        }
        Ok(Self::Command(text.parse::<Command>()?))
    }

    /// The files this edit imports and can be staged before it runs.
    pub(crate) fn import_sources(&self) -> Vec<String> {
        match self {
            Self::Command(command) => command_import_source(command)
                .map(str::to_owned)
                .into_iter()
                .collect(),
            Self::Group(actions) => transaction_import_sources(actions),
        }
    }

    pub(crate) fn perform(
        self,
        host: &DocumentHost,
        imports: StagedImports,
    ) -> Result<(), DocumentError> {
        match self {
            Self::Command(command) => {
                host.perform_staged(command, imports)?;
            }
            Self::Group(actions) => {
                host.apply_staged(actions, imports)?;
            }
        }
        Ok(())
    }
}

/// Imports being staged on their own thread. Dropping this stops the staging
/// and waits for it, and the staged bytes are removed with the result.
pub(crate) struct Staging {
    result: Receiver<StagedImports>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Staging {
    pub(crate) fn start(
        stage: &Stage,
        directory: PathBuf,
        sources: Vec<String>,
    ) -> Result<Self, DocumentError> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancel);
        let stage = Arc::clone(stage);
        let thread = thread::Builder::new()
            .name("echode-import-stage".to_string())
            .spawn(move || {
                let staged = stage(&directory, &sources, &|| !stopped.load(Ordering::Acquire));
                // A worker that has gone drops these, removing the staged bytes.
                let _ = sender.send(staged);
            })
            .map_err(DocumentError::Thread)?;
        Ok(Self {
            result,
            cancel,
            thread: Some(thread),
        })
    }

    /// The staged imports once the thread has finished.
    pub(crate) fn finished(&self) -> Option<Result<StagedImports, DocumentError>> {
        match self.result.try_recv() {
            Ok(imports) => Some(Ok(imports)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(DocumentError::ImportPanicked)),
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
