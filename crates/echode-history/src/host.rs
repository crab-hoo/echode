//! One thread owns a document. Callers send commands and read a snapshot.

use std::collections::HashSet;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};

use echode_actions::{
    Action, ActionError, Command, JournalLine, JournalOp, Session, parameter_message,
};
use echode_model::{ChangeSet, Id, Project};
use gix::ObjectId;
use serde_json::{Map, Value};

use crate::autosave::{Autosave, PendingAutosave, SavedRevision};
use crate::document::{
    JOURNAL_DURABILITY_UNKNOWN, PackageError, append_journal, autosave_tip, compact_journal,
    next_journal_seq, repair_journal,
};

/// Canonical package paths open in this process. `flock` does not stop a second open here.
static OPEN_PACKAGES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

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
    /// Plugin bytes already written to the object store. This is not a user edit.
    AbsorbPluginState {
        device: Id,
        state: String,
        reply: Sender<Result<bool, HostError>>,
    },
    Revisions {
        reply: Sender<Result<Vec<SavedRevision>, HostError>>,
    },
    Shutdown,
}

/// The only owner of a package's [`Session`].
pub struct DocumentHost {
    request: Sender<Request>,
    thread: Option<JoinHandle<()>>,
    snapshot: Arc<Mutex<Arc<Project>>>,
    lock: PackageLock,
    /// Commit recovery read, and the last journal line included in that project.
    base_revision: String,
    applied_seq: u64,
}

/// Exclusive ownership of one package, in this process and across processes.
struct PackageLock {
    file: File,
    path: PathBuf,
}

impl PackageLock {
    fn acquire(directory: &Path) -> Result<Self, PackageError> {
        let path = directory
            .canonicalize()
            .map_err(|error| PackageError::new(error.to_string()))?;
        {
            let mut open = OPEN_PACKAGES
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !open.insert(path.clone()) {
                return Err(PackageError::new("package is already open"));
            }
        }
        match lock_file(&path) {
            Ok(file) => Ok(Self { file, path }),
            Err(error) => {
                OPEN_PACKAGES
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&path);
                Err(error)
            }
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PackageLock {
    fn drop(&mut self) {
        OPEN_PACKAGES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
        let _ = self.file.unlock();
    }
}

fn lock_file(directory: &Path) -> Result<File, PackageError> {
    let path = directory.join("host.lock");
    let file = crate::document::open_package(directory)?
        .open_file("host.lock", true, true)
        .map_err(|error| PackageError::new(format!("{}: {error}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(PackageError::new("package is already open")),
        Err(TryLockError::Error(error)) => Err(PackageError::new(error.to_string())),
    }
}

fn device_state(project: &Project, device: Id) -> Option<Option<String>> {
    project.tracks().iter().find_map(|track| {
        track
            .devices
            .iter()
            .find(|item| item.id == device)
            .map(|item| item.state.clone())
    })
}

impl DocumentHost {
    /// Recovers `directory` and starts its document thread.
    ///
    /// A second open of the same package, including through a symbolic link,
    /// fails until this host is dropped. The lock file is held for the process
    /// lifetime so another process cannot write the package either.
    pub fn open(directory: &Path) -> Result<Self, PackageError> {
        let lock = PackageLock::acquire(directory)?;
        let directory = lock.path().to_path_buf();
        repair_journal(&directory)?;
        let recovered = crate::document::recover_labeled(&directory)?;
        let mut base_revision = recovered.base_revision;
        let applied_seq = recovered.applied_seq;
        let project = recovered.project;
        let snapshot = Arc::new(Mutex::new(Arc::new(project.clone())));
        let autosave = Autosave::create(&directory)?;
        // A confirmed edit is journaled before it returns. The first autosave
        // can still die with `save.pending` and no tip, and then recover
        // refuses the worktree. Commit the recovered project first, so a later
        // kill always has a tip to replay onto. That new commit is the base.
        if autosave_tip(&directory).is_none() {
            let id = autosave
                .save(Arc::new(project.clone()), applied_seq)?
                .wait()?;
            base_revision = id.to_string();
        }
        let (request, incoming) = mpsc::channel();
        let published = Arc::clone(&snapshot);
        let owned = directory.clone();
        let thread = thread::Builder::new()
            .name("echode-document".to_string())
            .spawn(move || worker(owned, project, autosave, published, incoming))
            .map_err(|error| PackageError::new(error.to_string()))?;
        Ok(Self {
            request,
            thread: Some(thread),
            snapshot,
            lock,
            base_revision,
            applied_seq,
        })
    }

    /// Package directory this host recovered. The path is canonical.
    pub fn directory(&self) -> &Path {
        self.lock.path()
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

    /// Writes plugin bytes, then records their hash on `device`.
    ///
    /// `Ok(true)` means the hash is in the journal, or it was already there.
    /// `Ok(false)` means the device is gone and the bytes must not be applied.
    /// `Err` means the object or the journal did not finish; the caller keeps the bytes.
    pub fn commit_plugin_state(&self, device: Id, bytes: &[u8]) -> Result<bool, HostError> {
        let hash = crate::write_object(self.directory(), bytes).map_err(HostError::Package)?;
        let project = self.snapshot();
        let Some(current) = device_state(&project, device) else {
            return Ok(false);
        };
        if current.as_deref() == Some(hash.as_str()) {
            return Ok(true);
        }
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::AbsorbPluginState {
                device,
                state: hash,
                reply,
            })
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::new("document thread stopped")))?
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

    /// Autosave tip and the journal sequence included in the recovered project.
    ///
    /// The tip alone is an earlier commit when later journal lines were replayed
    /// onto it. The pair identifies the snapshot [`Self::snapshot`] returns.
    pub fn recovered_label(&self) -> Result<(String, u64), HostError> {
        Ok((self.base_revision.clone(), self.applied_seq))
    }

    /// Autosave commits reachable from the tip, newest first.
    ///
    /// Saves still in flight are finished first, so the list includes them.
    pub fn revisions(&self) -> Result<Vec<SavedRevision>, HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Revisions { reply })
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
    let mut halted: Option<String> = None;
    while let Ok(request) = incoming.recv() {
        finish_saves(&directory, &mut inflight);
        if halted.is_some() && !matches!(request, Request::Shutdown | Request::Revisions { .. }) {
            reply_halted(request, &halted);
            continue;
        }
        match request {
            Request::Shutdown => break,
            Request::Perform { command, reply } => {
                let result = match command {
                    Command::Restore { revision } => {
                        wait_for_saves(&directory, &mut inflight);
                        let parked = session.park_redo();
                        match restore_revision(&mut session, &autosave, &revision) {
                            Ok(changes) if changes.is_empty() => {
                                session.unpark_redo(parked);
                                Ok(Action::Restore { revision })
                            }
                            Ok(_) => {
                                let saved = publish_edit(&mut Publish {
                                    directory: &directory,
                                    snapshot: &snapshot,
                                    autosave: &autosave,
                                    inflight: &mut inflight,
                                    next_seq: &mut next_seq,
                                    session: &session,
                                });
                                settle(&mut session, parked, &mut halted, saved)
                                    .map(|()| Action::Restore { revision })
                            }
                            Err(error) => {
                                session.unpark_redo(parked);
                                Err(error)
                            }
                        }
                    }
                    Command::ImportAudio { track, start, path } => {
                        match crate::media::import_audio(&directory, Path::new(&path)) {
                            Ok(stored) => {
                                let parked = session.park_redo();
                                match session.perform(Command::PlaceAudio {
                                    track,
                                    start,
                                    hash: stored.hash,
                                    sample_rate: stored.sample_rate,
                                    channels: stored.channels,
                                    frames: stored.frames,
                                }) {
                                    Ok(action) => {
                                        let saved = publish_edit(&mut Publish {
                                            directory: &directory,
                                            snapshot: &snapshot,
                                            autosave: &autosave,
                                            inflight: &mut inflight,
                                            next_seq: &mut next_seq,
                                            session: &session,
                                        });
                                        settle(&mut session, parked, &mut halted, saved)
                                            .map(|()| action)
                                    }
                                    Err(error) => {
                                        session.unpark_redo(parked);
                                        Err(HostError::Action(error))
                                    }
                                }
                            }
                            Err(error) => Err(HostError::Package(error)),
                        }
                    }
                    command => {
                        let parked = session.park_redo();
                        match session.perform(command) {
                            Ok(action) => {
                                let saved = publish_edit(&mut Publish {
                                    directory: &directory,
                                    snapshot: &snapshot,
                                    autosave: &autosave,
                                    inflight: &mut inflight,
                                    next_seq: &mut next_seq,
                                    session: &session,
                                });
                                settle(&mut session, parked, &mut halted, saved).map(|()| action)
                            }
                            Err(error) => {
                                session.unpark_redo(parked);
                                Err(HostError::Action(error))
                            }
                        }
                    }
                };
                let _ = reply.send(result);
            }
            Request::Apply { actions, reply } => {
                let parked = session.park_redo();
                let result = match apply_transaction(&mut session, actions) {
                    Ok(applied) if applied.is_empty() => {
                        session.unpark_redo(parked);
                        Ok(applied)
                    }
                    Ok(applied) => {
                        let saved = publish_edit(&mut Publish {
                            directory: &directory,
                            snapshot: &snapshot,
                            autosave: &autosave,
                            inflight: &mut inflight,
                            next_seq: &mut next_seq,
                            session: &session,
                        });
                        settle(&mut session, parked, &mut halted, saved).map(|()| applied)
                    }
                    Err(error) => {
                        session.unpark_redo(parked);
                        Err(error)
                    }
                };
                let _ = reply.send(result);
            }
            Request::Undo { reply } => {
                let before = session.project().clone();
                let result = match session.undo() {
                    Ok(stored) => {
                        let after = session.project().clone();
                        let changes = connecting_changeset(&after, &before, stored);
                        let saved = publish(
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
                        );
                        invert_on_failure(&mut session, &mut halted, saved, true)
                    }
                    Err(error) => Err(HostError::Action(error)),
                };
                let _ = reply.send(result);
            }
            Request::Revisions { reply } => {
                wait_for_saves(&directory, &mut inflight);
                let _ = reply.send(autosave.revisions().map_err(HostError::Package));
            }
            Request::Redo { reply } => {
                let before = session.project().clone();
                let result = match session.redo() {
                    Ok(stored) => {
                        let after = session.project().clone();
                        let changes = connecting_changeset(&before, &after, stored);
                        let saved = publish(
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
                        );
                        invert_on_failure(&mut session, &mut halted, saved, false)
                    }
                    Err(error) => Err(HostError::Action(error)),
                };
                let _ = reply.send(result);
            }
            Request::AbsorbPluginState {
                device,
                state,
                reply,
            } => {
                let previous = session.project().clone();
                let result = match session.absorb_plugin_state(device, Some(state.clone())) {
                    Ok(None) => Ok(true),
                    Ok(Some(changes)) => {
                        let saved = publish(
                            &mut Publish {
                                directory: &directory,
                                snapshot: &snapshot,
                                autosave: &autosave,
                                inflight: &mut inflight,
                                next_seq: &mut next_seq,
                                session: &session,
                            },
                            JournalOp::Edit,
                            vec![Action::SetPluginState {
                                device,
                                state: Some(state),
                            }],
                            changes,
                        );
                        match saved {
                            Ok(()) => Ok(true),
                            Err(error) => {
                                session.reinstall_current(previous);
                                note_failure(&mut halted, &error);
                                Err(error)
                            }
                        }
                    }
                    Err(error) => {
                        if device_state(session.project(), device).is_none() {
                            Ok(false)
                        } else {
                            Err(HostError::Action(error))
                        }
                    }
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

fn note_failure(halted: &mut Option<String>, error: &HostError) {
    if error.to_string().contains(JOURNAL_DURABILITY_UNKNOWN) {
        *halted = Some(error.to_string());
    }
}

/// Keeps a published edit. A failed publish removes that edit and restores the
/// redo stacks that were parked, without copying the undo history.
fn settle(
    session: &mut Session,
    parked: echode_actions::ParkedRedo,
    halted: &mut Option<String>,
    result: Result<(), HostError>,
) -> Result<(), HostError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            note_failure(halted, &error);
            let _ = session.revert_commit(parked);
            Err(error)
        }
    }
}

/// Undo and redo invert each other. `undo` is true when the failed publish
/// followed `Session::undo`, so `redo` puts that step back.
fn invert_on_failure(
    session: &mut Session,
    halted: &mut Option<String>,
    result: Result<(), HostError>,
    undo: bool,
) -> Result<(), HostError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            note_failure(halted, &error);
            let restored = if undo {
                session.redo().map(|_| ())
            } else {
                session.undo().map(|_| ())
            };
            if restored.is_err() {
                *halted = Some(JOURNAL_DURABILITY_UNKNOWN.to_string());
            }
            Err(error)
        }
    }
}

fn reply_halted(request: Request, halted: &Option<String>) {
    let message = halted
        .clone()
        .unwrap_or_else(|| JOURNAL_DURABILITY_UNKNOWN.to_string());
    let error = HostError::Package(PackageError::new(message));
    match request {
        Request::Perform { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Request::Apply { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Request::Undo { reply } | Request::Redo { reply } => {
            let _ = reply.send(Err(error));
        }
        Request::AbsorbPluginState { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Request::Revisions { reply } => {
            let _ = reply.send(Err(error));
        }
        Request::Shutdown => {}
    }
}

/// Uses `stored` when it still connects the two snapshots. An absorbed plugin
/// hash changes the current project without an undo step, so undo and redo
/// then publish the snapshot difference instead of the older changeset.
fn connecting_changeset(from: &Project, to: &Project, stored: ChangeSet) -> ChangeSet {
    if from.apply_changeset(&stored).ok().as_ref() == Some(to) {
        stored
    } else {
        from.diff(to)
    }
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

fn wait_for_saves(directory: &Path, inflight: &mut Vec<(u64, PendingAutosave)>) {
    for (seq, pending) in inflight.drain(..) {
        if pending.wait().is_ok() {
            let _ = compact_journal(directory, seq);
        }
    }
}

fn restore_revision(
    session: &mut Session,
    autosave: &Autosave,
    revision: &str,
) -> Result<echode_model::ChangeSet, HostError> {
    let id = ObjectId::from_hex(revision.as_bytes())
        .map_err(|_| HostError::Package(PackageError::new("saved revision was not found")))?;
    let project = autosave.load(id).map_err(HostError::Package)?;
    session
        .restore(project, revision.to_string())
        .map_err(HostError::Action)
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
    use crate::document::JournalFault;
    use crate::document::write_package;
    use crate::recover;
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

    #[test]
    fn restoring_an_older_autosave_records_a_new_version_and_undoes() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack { name: "one".into() })
            .unwrap();
        assert_eq!(host.revisions().unwrap().len(), 2);
        host.perform(Command::AddTrack { name: "two".into() })
            .unwrap();
        let revisions = host.revisions().unwrap();
        assert_eq!(revisions.len(), 3);
        // Newest first. The last commit is the empty base; the one before it
        // is the project after the first track.
        let after_first = &revisions[revisions.len() - 2];
        host.perform(Command::Restore {
            revision: after_first.id.clone(),
        })
        .unwrap();
        assert_eq!(host.snapshot().tracks().len(), 1);
        assert_eq!(host.snapshot().tracks()[0].name, "one");
        let kept = after_first.id.clone();
        host.undo().unwrap();
        assert_eq!(host.snapshot().tracks().len(), 2);
        host.redo().unwrap();
        assert_eq!(host.snapshot().tracks()[0].name, "one");
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let names: Vec<_> = again
            .snapshot()
            .tracks()
            .iter()
            .map(|track| track.name.clone())
            .collect();
        let still = again.revisions().unwrap();
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(names, ["one"]);
        assert!(still.iter().any(|revision| revision.id == kept));
    }

    fn names(host: &DocumentHost) -> Vec<String> {
        host.snapshot()
            .tracks()
            .iter()
            .map(|track| track.name.clone())
            .collect()
    }

    #[test]
    fn audit_aud01_a_second_open_in_this_process_is_rejected() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack { name: "A".into() })
            .unwrap();
        let second = DocumentHost::open(&directory);
        assert!(second.is_err(), "a second writer must be rejected");
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(recovered, ["A"]);
    }

    #[test]
    fn audit_aud01_a_symlink_is_the_same_package() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let link = scratch();
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        let through_link = DocumentHost::open(&link);
        drop(host);
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(directory);
        assert!(
            through_link.is_err(),
            "a symlink must not open a second writer"
        );
    }

    #[test]
    fn audit_aud01_another_process_cannot_open_and_a_forced_exit_keeps_the_edit() {
        if let Ok(directory) = std::env::var("ECHODE_AUDIT_HOLD") {
            let host = DocumentHost::open(std::path::Path::new(&directory)).unwrap();
            println!("holding");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let mut byte = [0u8; 1];
            let _ = std::io::Read::read(&mut std::io::stdin(), &mut byte);
            host.perform(Command::AddTrack {
                name: "kept".into(),
            })
            .unwrap();
            // Safety: the journal line was synced before this exit. Drop is not required.
            unsafe { libc::_exit(0) };
        }
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("ECHODE_AUDIT_HOLD", &directory)
            .arg("--exact")
            .arg("host::tests::audit_aud01_another_process_cannot_open_and_a_forced_exit_keeps_the_edit")
            .arg("--nocapture")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut seen = String::new();
        let mut buffer = [0u8; 64];
        while !seen.contains("holding") {
            let read = std::io::Read::read(&mut stdout, &mut buffer).unwrap();
            assert!(read > 0, "the holder exited before it owned the package");
            seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
        let rejected = DocumentHost::open(&directory);
        let link = scratch();
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        let rejected_link = DocumentHost::open(&link);
        assert!(
            rejected.is_err(),
            "another process must not become a writer"
        );
        assert!(rejected_link.is_err(), "a symlink must not become a writer");
        let _ = std::fs::remove_file(&link);
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), &[1]).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(recovered, ["kept"]);
    }

    #[test]
    fn audit_aud02_every_torn_offset_can_be_edited_and_reopened() {
        let source = scratch();
        write_package(&source, &Project::new()).unwrap();
        let full = {
            let host = DocumentHost::open(&source).unwrap();
            host.perform(Command::AddTrack {
                name: "ピアノ".into(),
            })
            .unwrap();
            let bytes = std::fs::read(source.join("journal.jsonl")).unwrap();
            drop(host);
            bytes
        };
        let _ = std::fs::remove_dir_all(source);
        assert!(full.ends_with(b"\n"));
        let text = std::str::from_utf8(&full).unwrap();
        assert!(text.contains("ピアノ"));
        let line = echode_actions::decode_line(text.lines().next().unwrap()).unwrap();
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        {
            let host = DocumentHost::open(&directory).unwrap();
            drop(host);
        }
        let log = directory.join("journal.jsonl");
        // Each offset is repaired, reopened, and given one more synced record.
        // DocumentHost::open and perform use these same steps.
        for cut in 0..=full.len() {
            std::fs::write(&log, &full[..cut]).unwrap();
            crate::document::repair_journal(&directory).unwrap();
            let opened = recover(&directory).unwrap();
            if !opened.tracks().iter().any(|track| track.name == "ピアノ") {
                crate::document::append_journal(&directory, &line).unwrap();
            }
            let again = recover(&directory).unwrap();
            assert!(
                again.tracks().iter().any(|track| track.name == "ピアノ"),
                "cut {cut} of {} dropped the committed edit",
                full.len()
            );
        }
        let mut torn = full.clone();
        torn.push(0xE3);
        std::fs::write(&log, &torn).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack {
            name: "確定".into(),
        })
        .unwrap();
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert!(recovered.iter().any(|name| name == "ピアノ"));
        assert!(recovered.iter().any(|name| name == "確定"));
    }

    #[test]
    fn audit_aud03_a_failed_journal_write_is_not_part_of_the_next_edit() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        let log = directory.join("journal.jsonl");
        if log.exists() {
            std::fs::remove_file(&log).unwrap();
        }
        std::fs::create_dir(&log).unwrap();
        let failed = host.perform(Command::AddTrack {
            name: "rejected".into(),
        });
        assert!(failed.is_err());
        assert!(names(&host).is_empty());
        std::fs::remove_dir(&log).unwrap();
        host.perform(Command::AddTrack {
            name: "accepted".into(),
        })
        .unwrap();
        assert_eq!(names(&host), ["accepted"]);
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(recovered, ["accepted"]);
    }

    #[test]
    fn audit_aud03_a_partial_write_and_a_sync_failure_roll_back() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack {
            name: "kept".into(),
        })
        .unwrap();
        for fault in [
            JournalFault::Partial,
            JournalFault::Sync,
            JournalFault::BeforeWrite,
        ] {
            crate::document::fail_next_journal(host.directory(), fault);
            let failed = host.perform(Command::AddTrack {
                name: "rejected".into(),
            });
            assert!(failed.is_err(), "{fault:?}");
            assert_eq!(names(&host), ["kept"]);
            host.undo().unwrap();
            assert!(names(&host).is_empty());
            host.redo().unwrap();
            assert_eq!(names(&host), ["kept"]);
        }
        host.perform(Command::AddTrack {
            name: "accepted".into(),
        })
        .unwrap();
        assert_eq!(names(&host), ["kept", "accepted"]);
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(recovered, ["kept", "accepted"]);
    }

    #[test]
    fn audit_aud03_unknown_durability_stops_later_edits_until_recovery() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        crate::document::fail_next_journal(host.directory(), JournalFault::Uncertain);
        let failed = host.perform(Command::AddTrack {
            name: "landed".into(),
        });
        assert!(
            failed
                .unwrap_err()
                .to_string()
                .contains("journal durability is unknown")
        );
        assert!(names(&host).is_empty());
        let next = host.perform(Command::AddTrack {
            name: "mixed".into(),
        });
        assert!(
            next.unwrap_err()
                .to_string()
                .contains("journal durability is unknown")
        );
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let recovered = names(&again);
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(recovered, ["landed"]);
    }

    #[test]
    fn audit_rea09_export_label_uses_readable_parent() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
        let parent = host.revisions().unwrap().into_iter().next().unwrap().id;
        host.perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap();
        let tip = host.revisions().unwrap().into_iter().next().unwrap().id;
        assert_ne!(parent, tip);
        drop(host);
        let repo = gix::open(&directory).unwrap();
        let commit = repo
            .find_commit(gix::ObjectId::from_hex(tip.as_bytes()).unwrap())
            .unwrap();
        let blob = commit
            .tree()
            .unwrap()
            .lookup_entry_by_path("project.json")
            .unwrap()
            .unwrap()
            .object_id();
        let hex = blob.to_string();
        std::fs::remove_file(
            directory
                .join(".git/objects")
                .join(&hex[..2])
                .join(&hex[2..]),
        )
        .unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        let (id, _) = host.recovered_label().unwrap();
        assert_eq!(host.snapshot().tracks().len(), 1);
        assert_eq!(host.snapshot().tracks()[0].name, "piano");
        assert_eq!(id, parent);
        assert_ne!(id, tip);
        drop(host);
        let _ = std::fs::remove_dir_all(directory);
    }

    fn clap(project: &Project) -> &echode_model::Device {
        project.tracks()[0]
            .devices
            .iter()
            .find(|device| device.plugin.as_deref() == Some("dev.echode.capture"))
            .unwrap()
    }

    #[test]
    fn a_captured_plugin_state_keeps_redo_and_does_not_add_an_undo_step() {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack { name: "a".into() })
            .unwrap();
        let track = host.snapshot().tracks()[0].id;
        host.perform(Command::AddDevice {
            track,
            plugin: "dev.echode.capture".into(),
        })
        .unwrap();
        host.perform(Command::AddTrack { name: "b".into() })
            .unwrap();
        host.undo().unwrap();
        assert_eq!(host.snapshot().tracks().len(), 1);
        let device = clap(&host.snapshot()).id;
        assert!(host.commit_plugin_state(device, b"captured-state").unwrap());
        host.redo().unwrap();
        assert_eq!(host.snapshot().tracks().len(), 2);
        assert_eq!(host.snapshot().tracks()[1].name, "b");
        drop(host);
        let _ = std::fs::remove_dir_all(directory);

        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let host = DocumentHost::open(&directory).unwrap();
        host.perform(Command::AddTrack { name: "a".into() })
            .unwrap();
        let track = host.snapshot().tracks()[0].id;
        host.perform(Command::AddDevice {
            track,
            plugin: "dev.echode.capture".into(),
        })
        .unwrap();
        let device = clap(&host.snapshot()).id;
        assert!(host.commit_plugin_state(device, b"captured-state").unwrap());
        let saved = clap(&host.snapshot()).state.clone();
        host.undo().unwrap();
        assert!(
            host.snapshot().tracks()[0]
                .devices
                .iter()
                .all(|device| device.plugin.is_none())
        );
        host.undo().unwrap();
        assert!(host.snapshot().tracks().is_empty());
        assert!(host.undo().is_err());
        host.redo().unwrap();
        host.redo().unwrap();
        drop(host);
        let again = DocumentHost::open(&directory).unwrap();
        let project = again.snapshot();
        drop(again);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(clap(&project).state, saved);
    }
}
