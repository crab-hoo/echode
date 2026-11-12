//! External audio decoded before its edit reaches the document thread.
//!
//! Decoding and writing a long file takes hundreds of milliseconds, and the
//! document thread would hold every later turn meanwhile. Staging touches no
//! history: it writes only an unnamed file beside the store. So a caller
//! stages an edit's imports on its own thread, then hands them to
//! [`crate::DocumentHost::perform_staged`] or
//! [`crate::DocumentHost::apply_staged`] with the edit. The edit is checked
//! and published exactly as if the document thread had staged them (ADR 0331).

use std::path::Path;

use echode_actions::{Action, Command};
use echode_model::{Id, Tick};
use serde_json::Value;

use crate::document::PackageError;
use crate::media::{StagedAudio, stage_audio};

/// Each staged import, in the order the edit names its sources. An entry the
/// edit never uses is dropped with its staged bytes.
#[derive(Default)]
pub struct StagedImports {
    entries: Vec<(String, Result<StagedAudio, PackageError>)>,
}

impl StagedImports {
    /// Nothing staged: the document thread stages each import itself.
    pub fn none() -> Self {
        Self::default()
    }

    /// Decodes each source and writes its PCM beside the store. A failure is
    /// kept for its source, and the edit reports it where it names that
    /// source. Staging stops once `keep_going` is false; a source left
    /// unstaged is staged by the document thread if the edit still runs.
    pub fn stage(package: &Path, sources: &[String], keep_going: &dyn Fn() -> bool) -> Self {
        let mut entries = Vec::with_capacity(sources.len());
        for source in sources {
            if !keep_going() {
                break;
            }
            entries.push((
                source.clone(),
                stage_audio(package, Path::new(source), keep_going),
            ));
        }
        Self { entries }
    }

    /// How many sources were staged, failures included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The first unused stage of `source`, or one staged now on this thread.
    pub(crate) fn take_or_stage(
        &mut self,
        package: &Path,
        source: &str,
    ) -> Result<StagedAudio, PackageError> {
        match self.entries.iter().position(|(staged, _)| staged == source) {
            Some(index) => self.entries.remove(index).1,
            None => stage_audio(package, Path::new(source), &|| true),
        }
    }
}

/// The file `command` imports, if it is an import.
pub fn command_import_source(command: &Command) -> Option<&str> {
    match command {
        Command::ImportAudio { path, .. } => Some(path),
        _ => None,
    }
}

/// The files a transaction imports, in order. A source given as a reference to
/// an earlier result is resolved only when the edit runs, so the document
/// thread stages that one itself.
pub fn transaction_import_sources(actions: &[Value]) -> Vec<String> {
    let kind = import_kind();
    actions
        .iter()
        .filter_map(|action| {
            let action = action.as_object()?;
            if action.get("type")?.as_str()? != kind {
                return None;
            }
            action.get("path")?.as_str().map(str::to_owned)
        })
        .collect()
}

/// The wire name of an import, read from the action registry.
fn import_kind() -> &'static str {
    Action::ImportAudio {
        track: Id::from_raw(0),
        start: Tick::new(0),
        path: String::new(),
    }
    .spec()
    .id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transaction_names_only_its_literal_import_sources_in_order() {
        let actions = serde_json::json!([
            {"type": "track.add", "name": "a.wav"},
            {"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": "/one.wav"},
            {"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": {"ref": 0, "field": "name"}},
            {"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 9, "path": "/two.wav"},
        ]);
        let Value::Array(actions) = actions else {
            unreachable!()
        };
        assert_eq!(
            transaction_import_sources(&actions),
            ["/one.wav", "/two.wav"]
        );
        let command: Command =
            r#"{"type":"audio.import","track":"00000000-0000-0000-0000-000000000001","start":0,"path":"/x.wav"}"#
                .parse()
                .unwrap();
        assert_eq!(command_import_source(&command), Some("/x.wav"));
    }
}
