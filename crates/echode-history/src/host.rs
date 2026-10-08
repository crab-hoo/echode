//! One thread owns a document. Callers send commands and read a snapshot.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use echode_actions::{
    Action, ActionError, Command, JournalLine, JournalOp, Session, parameter_message,
};
use echode_model::{ChangeSet, Project};
use serde_json::{Map, Value};

use crate::autosave::{Autosave, PendingAutosave};
use crate::document::{PackageError, append_journal, compact_journal, next_journal_seq, recover};

/// Why the document thread rejected a command.
#[derive(Debug)]
pub enum HostError {
    Action(ActionError),
    Package(PackageError),
    /// One element of a transaction failed. `parameters` selects the JSON-RPC code.
    Element {
        index: usize,
        parameters: bool,
        message: String,
    },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Action(error) => write!(formatter, "{error}"),
            Self::Package(error) => write!(formatter, "{error}"),
            Self::Element { message, .. } => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for HostError {}

enum Request {
    Perform {
        command: Command,
        reply: Sender<Result<Action, HostError>>,
    },
    Apply {
        actions: Vec<Value>,
        reply: Sender<Result<Vec<Action>, HostError>>,
    },
    Undo {
        reply: Sender<Result<(), HostError>>,
    },
    Redo {
        reply: Sender<Result<(), HostError>>,
    },
    Shutdown,
}

/// The only owner of a package's [`Session`].
pub struct DocumentHost {
    request: Sender<Request>,
    thread: Option<JoinHandle<()>>,
    snapshot: Arc<Mutex<Arc<Project>>>,
}

impl DocumentHost {
    /// Recovers `directory` and starts its document thread.
    pub fn open(directory: &Path) -> Result<Self, PackageError> {
        let project = recover(directory)?;
        let snapshot = Arc::new(Mutex::new(Arc::new(project.clone())));
        let autosave = Autosave::create(directory)?;
        let (request, incoming) = mpsc::channel();
        let published = Arc::clone(&snapshot);
        let directory = directory.to_path_buf();
        let thread = thread::Builder::new()
            .name("echode-document".to_string())
            .spawn(move || worker(directory, project, autosave, published, incoming))
            .map_err(|error| PackageError::new(error.to_string()))?;
        Ok(Self {
            request,
            thread: Some(thread),
            snapshot,
        })
    }

    /// The latest project. The lock is released before the caller uses it.
    pub fn snapshot(&self) -> Arc<Project> {
        Arc::clone(
            &self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Applies `command` on the document thread and returns after the journal is written.
    pub fn perform(&self, command: Command) -> Result<Action, HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Perform { command, reply })
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?
    }

    /// Applies every action as one undo step. References are resolved in order.
    pub fn apply(&self, actions: Vec<Value>) -> Result<Vec<Action>, HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Apply { actions, reply })
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?
    }

    /// Undoes the newest undo step and appends an `undo` journal line.
    pub fn undo(&self) -> Result<(), HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Undo { reply })
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?
    }

    /// Redoes the newest undone step and appends a `redo` journal line.
    pub fn redo(&self) -> Result<(), HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Redo { reply })
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?
    }
}

impl Drop for DocumentHost {
    fn drop(&mut self) {
        let _ = self.request.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker(
    directory: std::path::PathBuf,
    project: Project,
    autosave: Autosave,
    snapshot: Arc<Mutex<Arc<Project>>>,
    incoming: Receiver<Request>,
) {
    let mut session = Session::open(project);
    let mut next_seq = next_journal_seq(&directory).unwrap_or(1);
    let mut inflight: Vec<(u64, PendingAutosave)> = Vec::new();
    while let Ok(request) = incoming.recv() {
        finish_saves(&directory, &mut inflight);
        match request {
            Request::Shutdown => break,
            Request::Perform { command, reply } => {
                let result = match session.perform(command) {
                    Ok(action) => publish_edit(&mut Publish {
                        directory: &directory,
                        snapshot: &snapshot,
                        autosave: &autosave,
                        inflight: &mut inflight,
                        next_seq: &mut next_seq,
                        session: &session,
                    })
                    .map(|()| action),
                    Err(error) => Err(HostError::Action(error)),
                };
                let _ = reply.send(result);
            }
            Request::Apply { actions, reply } => {
                let result = match apply_transaction(&mut session, actions) {
                    Ok(applied) if applied.is_empty() => Ok(applied),
                    Ok(applied) => publish_edit(&mut Publish {
                        directory: &directory,
                        snapshot: &snapshot,
                        autosave: &autosave,
                        inflight: &mut inflight,
                        next_seq: &mut next_seq,
                        session: &session,
                    })
                    .map(|()| applied),
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            Request::Undo { reply } => {
                let result = match session.undo() {
                    Ok(changes) => publish(
                        &mut Publish {
                            directory: &directory,
                            snapshot: &snapshot,
                            autosave: &autosave,
                            inflight: &mut inflight,
                            next_seq: &mut next_seq,
                            session: &session,
                        },
                        JournalOp::Undo,
                        Vec::new(),
                        changes,
                    ),
                    Err(error) => Err(HostError::Action(error)),
                };
                let _ = reply.send(result);
            }
            Request::Redo { reply } => {
                let result = match session.redo() {
                    Ok(changes) => publish(
                        &mut Publish {
                            directory: &directory,
                            snapshot: &snapshot,
                            autosave: &autosave,
                            inflight: &mut inflight,
                            next_seq: &mut next_seq,
                            session: &session,
                        },
                        JournalOp::Redo,
                        Vec::new(),
                        changes,
                    ),
                    Err(error) => Err(HostError::Action(error)),
                };
                let _ = reply.send(result);
            }
        }
    }
    for (seq, pending) in inflight {
        if pending.wait().is_ok() {
            let _ = compact_journal(&directory, seq);
        }
    }
}

struct Publish<'a> {
    directory: &'a Path,
    snapshot: &'a Arc<Mutex<Arc<Project>>>,
    autosave: &'a Autosave,
    inflight: &'a mut Vec<(u64, PendingAutosave)>,
    next_seq: &'a mut u64,
    session: &'a Session,
}

fn publish_edit(commit: &mut Publish<'_>) -> Result<(), HostError> {
    let entry = commit
        .session
        .journal()
        .last()
        .expect("a committed edit records a journal entry");
    let actions = entry.actions.clone();
    let changes = entry.changes.clone().unwrap_or_default();
    publish(commit, JournalOp::Edit, actions, changes)
}

fn publish(
    commit: &mut Publish<'_>,
    op: JournalOp,
    actions: Vec<Action>,
    changes: ChangeSet,
) -> Result<(), HostError> {
    let line = JournalLine {
        seq: *commit.next_seq,
        op,
        actions,
        changes,
    };
    append_journal(commit.directory, &line).map_err(HostError::Package)?;
    *commit.next_seq = commit.next_seq.saturating_add(1);
    let published = Arc::new(commit.session.project().clone());
    *commit
        .snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::clone(&published);
    if let Ok(pending) = commit.autosave.save(published, line.seq) {
        commit.inflight.push((line.seq, pending));
    }
    Ok(())
}

fn apply_transaction(session: &mut Session, actions: Vec<Value>) -> Result<Vec<Action>, HostError> {
    let mut applied = Vec::with_capacity(actions.len());
    let mut prior = Vec::with_capacity(actions.len());
    let mut failure: Option<HostError> = None;
    let result = session.transaction(|transaction| {
        for (index, action) in actions.iter().enumerate() {
            let mut value = action.clone();
            if let Err(message) = resolve_refs(&mut value, &prior) {
                failure = Some(HostError::Element {
                    index,
                    parameters: true,
                    message,
                });
                return Err(ActionError::CorruptJournal);
            }
            let command = match serde_json::from_value(value) {
                Ok(command) => command,
                Err(error) => {
                    failure = Some(HostError::Element {
                        index,
                        parameters: true,
                        message: parameter_message(&error),
                    });
                    return Err(ActionError::CorruptJournal);
                }
            };
            match transaction.perform(command) {
                Ok(action) => {
                    prior.push(serde_json::to_value(&action).expect("a committed action is json"));
                    applied.push(action);
                }
                Err(error) => {
                    failure = Some(HostError::Element {
                        index,
                        parameters: false,
                        message: error.to_string(),
                    });
                    return Err(error);
                }
            }
        }
        Ok(())
    });
    match result {
        Ok(()) => Ok(applied),
        Err(_) => Err(failure.expect("a failed transaction records the element")),
    }
}

fn resolve_refs(value: &mut Value, prior: &[Value]) -> Result<(), String> {
    if let Some(replacement) = reference(value, prior)? {
        *value = replacement;
        return Ok(());
    }
    match value {
        Value::Array(items) => {
            for item in items {
                resolve_refs(item, prior)?;
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                resolve_refs(child, prior)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn reference(value: &Value, prior: &[Value]) -> Result<Option<Value>, String> {
    let Value::Object(map) = value else {
        return Ok(None);
    };
    if !is_reference(map) {
        return Ok(None);
    }
    let Some(index) = map.get("ref").and_then(Value::as_u64) else {
        return Err("action reference is not an index".to_string());
    };
    let Some(field) = map.get("field").and_then(Value::as_str) else {
        return Err("action reference is missing a field".to_string());
    };
    let Some(index) = usize::try_from(index).ok() else {
        return Err("action reference is out of range".to_string());
    };
    let Some(earlier) = prior.get(index) else {
        return Err("action reference is out of range".to_string());
    };
    let Some(found) = earlier.get(field).and_then(Value::as_str) else {
        return Err("action reference field is missing".to_string());
    };
    Ok(Some(Value::String(found.to_string())))
}

fn is_reference(map: &Map<String, Value>) -> bool {
    map.len() == 2 && map.contains_key("ref") && map.contains_key("field")
}

fn finish_saves(directory: &Path, inflight: &mut Vec<(u64, PendingAutosave)>) {
    while let Some((seq, pending)) = inflight.first() {
        match pending.try_wait() {
            None => return,
            Some(Ok(_)) => {
                let seq = *seq;
                inflight.remove(0);
                let _ = compact_journal(directory, seq);
            }
            Some(Err(_)) => {
                inflight.remove(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::write_package;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("echode-host-{}-{n}", std::process::id()))
    }

    #[test]
    fn an_edit_is_still_there_after_the_host_is_dropped() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
        assert_eq!(host.snapshot().tracks()[0].name, "piano");
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let project = again.snapshot();
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(project.tracks().len(), 1);
        assert_eq!(project.tracks()[0].name, "piano");
    }
}
