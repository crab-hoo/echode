//! One thread owns a document. Callers send commands and read a snapshot.

use std::collections::HashSet;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};

use echode_actions::{Action, ActionError, Command, HistoryUsage, JournalLine, JournalOp, Session};
use echode_model::{ChangeSet, Id, Project};
use gix::ObjectId;
use serde_json::{Map, Value};

use crate::autosave::{Autosave, PendingAutosave, SavedRevision};
use crate::document::{
    PackageError, append_journal, autosave_tip, compact_journal, next_journal_seq, repair_journal,
};

/// Canonical package paths open in this process. `flock` does not stop a second open here.
static OPEN_PACKAGES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

use crate::error::DocumentError;
#[path = "host_error.rs"]
mod error;
pub use error::{ElementError, HostError, ReferenceError};

/// Recent edits may be missing from the opened document: a readable ancestor
/// was used instead of the autosave tip, or the journal could not be replayed
/// to its end. Editing requires consent (ADR 0183). `tip` equals `base` when
/// only the journal stopped early.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RecoveryNotice {
    pub tip: String,
    pub base: String,
    pub applied_seq: u64,
    pub reason: crate::RecoveryReason,
}

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
    /// Read-only, so a halted thread still answers it.
    Usage {
        reply: Sender<Result<HistoryUsage, HostError>>,
    },
    AcceptRecovery {
        reply: Sender<Result<(), HostError>>,
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
    recovery: Option<RecoveryNotice>,
    recovery_pending: Arc<AtomicBool>,
}

/// Exclusive ownership of one package, in this process and across processes.
struct PackageLock {
    file: File,
    path: PathBuf,
}

impl PackageLock {
    fn acquire(directory: &Path) -> Result<Self, PackageError> {
        let path = directory.canonicalize().map_err(PackageError::from)?;
        {
            let mut open = OPEN_PACKAGES
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !open.insert(path.clone()) {
                return Err(PackageError::Document(DocumentError::AlreadyOpen));
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
        .map_err(|error| crate::document::io_error(&path, error))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(PackageError::Document(DocumentError::AlreadyOpen)),
        Err(TryLockError::Error(error)) => Err(PackageError::from(error)),
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
        let recovery = recovered.reason.map(|reason| RecoveryNotice {
            tip: recovered.tip_revision.clone().unwrap_or_default(),
            base: recovered.base_revision.clone(),
            applied_seq: recovered.applied_seq,
            reason,
        });
        let recovery_pending = Arc::new(AtomicBool::new(recovery.is_some()));
        let package = package_name(&directory);
        match &recovery {
            Some(notice) => log::warn!(
                target: LOG,
                package = package.as_str(),
                reason:? = notice.reason,
                tip = notice.tip.as_str(),
                base = notice.base.as_str(),
                applied_seq = notice.applied_seq;
                "a document opened from an older state; editing waits for consent"
            ),
            None => log::info!(
                target: LOG,
                package = package.as_str(),
                base = recovered.base_revision.as_str(),
                applied_seq = recovered.applied_seq;
                "a document opened"
            ),
        }
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
        let pending = Arc::clone(&recovery_pending);
        let thread = thread::Builder::new()
            .name("echode-document".to_string())
            .spawn(move || {
                worker(
                    owned,
                    project,
                    autosave,
                    published,
                    incoming,
                    pending,
                    applied_seq,
                )
            })
            .map_err(PackageError::from)?;
        Ok(Self {
            request,
            thread: Some(thread),
            snapshot,
            lock,
            base_revision,
            applied_seq,
            recovery,
            recovery_pending,
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
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
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
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// Applies `command`, writing any nonempty edit to the journal before returning.
    /// A no-op succeeds without publishing or discarding redo.
    pub fn perform(&self, command: Command) -> Result<Action, HostError> {
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Perform { command, reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// Applies every action as one undo step. References are resolved in order.
    /// A group with no net change returns all accepted actions without recording a step.
    pub fn apply(&self, actions: Vec<Value>) -> Result<Vec<Action>, HostError> {
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Apply { actions, reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// Undoes the newest undo step and appends an `undo` journal line.
    pub fn undo(&self) -> Result<(), HostError> {
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Undo { reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// Pending consent is not an edit and never creates an Undo step.
    pub fn recovery_notice(&self) -> Option<&RecoveryNotice> {
        self.recovery
            .as_ref()
            .filter(|_| self.recovery_pending.load(Ordering::Acquire))
    }

    /// Every writer must check this before changing project or session state.
    pub fn require_recovery_confirmation(&self) -> Result<(), PackageError> {
        if let Some(notice) = self.recovery_notice() {
            return Err(PackageError::Document(
                DocumentError::RecoveryConfirmationRequired(notice.clone()),
            ));
        }
        Ok(())
    }

    /// Accepts only the exact warning shown to the caller. Checkpoints the chosen
    /// snapshot before enabling edits, so a crash cannot replay them onto the bad tip.
    pub fn accept_recovery(&self, tip: &str, base: &str) -> Result<(), HostError> {
        if !self
            .recovery
            .as_ref()
            .is_some_and(|notice| notice.tip == tip && notice.base == base)
        {
            return Err(HostError::Package(PackageError::Document(
                DocumentError::RecoveryMismatch,
            )));
        }
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::AcceptRecovery { reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
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
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// What undo and redo keep in memory beyond the current project (ADR 0286).
    ///
    /// The document thread walks its history, so the time grows with the
    /// number of steps. Call it for diagnosis, not on every frame.
    pub fn history_usage(&self) -> Result<HistoryUsage, HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Usage { reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    /// Redoes the newest undone step and appends a `redo` journal line.
    pub fn redo(&self) -> Result<(), HostError> {
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(Request::Redo { reply })
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
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
    recovery_pending: Arc<AtomicBool>,
    applied_seq: u64,
) {
    let mut session = Session::open(project);
    let mut next_seq = next_journal_seq(&directory)
        .unwrap_or(1)
        .max(applied_seq.saturating_add(1));
    let mut inflight: Vec<(u64, PendingAutosave)> = Vec::new();
    let mut halted = false;
    while let Ok(request) = incoming.recv() {
        finish_saves(&directory, &mut inflight);
        if halted
            && !matches!(
                request,
                Request::Shutdown | Request::Revisions { .. } | Request::Usage { .. }
            )
        {
            reply_halted(request);
            continue;
        }
        match request {
            Request::Shutdown => break,
            Request::AcceptRecovery { reply } => {
                let result = if recovery_pending.load(Ordering::Acquire) {
                    let seq = next_seq.saturating_sub(1);
                    autosave
                        .save(Arc::new(session.project().clone()), seq)
                        .and_then(|pending| pending.wait())
                        .and_then(|_| compact_journal(&directory, seq))
                        .map(|()| recovery_pending.store(false, Ordering::Release))
                        .map_err(HostError::Package)
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
            }
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
                        match crate::media::stage_audio(&directory, Path::new(&path)) {
                            Ok(staged) => {
                                let stored = staged.stored().clone();
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
                                        let saved = publish_with_media(
                                            &mut Publish {
                                                directory: &directory,
                                                snapshot: &snapshot,
                                                autosave: &autosave,
                                                inflight: &mut inflight,
                                                next_seq: &mut next_seq,
                                                session: &session,
                                            },
                                            vec![staged],
                                        );
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
                        let journal_len = session.journal().len();
                        let parked = session.park_redo();
                        match session.perform(command) {
                            Ok(action) if session.journal().len() == journal_len => {
                                session.unpark_redo(parked);
                                Ok(action)
                            }
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
                let journal_len = session.journal().len();
                let parked = session.park_redo();
                let mut staged = Vec::new();
                let result = match apply_transaction(&mut session, &directory, actions, &mut staged)
                {
                    Ok(applied) if session.journal().len() == journal_len => {
                        session.unpark_redo(parked);
                        Ok(applied)
                    }
                    Ok(applied) => {
                        let saved = publish_with_media(
                            &mut Publish {
                                directory: &directory,
                                snapshot: &snapshot,
                                autosave: &autosave,
                                inflight: &mut inflight,
                                next_seq: &mut next_seq,
                                session: &session,
                            },
                            std::mem::take(&mut staged),
                        );
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
            Request::Usage { reply } => {
                let _ = reply.send(Ok(session.usage()));
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
    // Read at close, where the walk over every version delays no edit (ADR 0286).
    let usage = session.usage();
    log::info!(
        target: LOG,
        package = package_name(&directory).as_str(),
        halted = halted,
        undo_steps = usage.undo_steps,
        redo_steps = usage.redo_steps,
        version_bytes = usage.versions.bytes,
        version_tracks = usage.versions.tracks,
        version_contents = usage.versions.contents,
        version_notes = usage.versions.notes,
        journal_changes = usage.journal_changes,
        journal_bytes = usage.journal_bytes;
        "a document closed"
    );
}

/// The log's target for the document thread.
const LOG: &str = "echode_history::host";

/// The package's file name. The log names the package, not its folder path.
fn package_name(directory: &Path) -> String {
    directory
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

struct Publish<'a> {
    directory: &'a Path,
    snapshot: &'a Arc<Mutex<Arc<Project>>>,
    autosave: &'a Autosave,
    inflight: &'a mut Vec<(u64, PendingAutosave)>,
    next_seq: &'a mut u64,
    session: &'a Session,
}

fn note_failure(halted: &mut bool, error: &HostError) {
    if error.durability_unknown() {
        *halted = true;
        log::error!(
            target: LOG,
            error:% = error,
            cause:? = error;
            "the document stopped taking edits: a write's durability is unknown"
        );
    } else {
        log::warn!(target: LOG, error:% = error, cause:? = error; "a document change was not saved");
    }
}

/// Keeps a published edit. A failed publish removes that edit and restores the
/// redo stacks that were parked, without copying the undo history.
fn settle(
    session: &mut Session,
    parked: echode_actions::ParkedRedo,
    halted: &mut bool,
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
    halted: &mut bool,
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
                *halted = true;
            }
            Err(error)
        }
    }
}

fn reply_halted(request: Request) {
    let error = HostError::Package(PackageError::durability_unknown());
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
        Request::Usage { reply } => {
            let _ = reply.send(Err(error));
        }
        Request::AcceptRecovery { reply } => {
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

/// Publishes the media an accepted edit imported, then its journal line.
///
/// The objects and their directory entries are synced before the line that
/// names them (XCA-03). When the line is cleanly not written, objects this
/// edit created are removed again; when its durability is unknown they stay,
/// because reopening may replay the line (EXT-02).
fn publish_with_media(
    commit: &mut Publish<'_>,
    staged: Vec<crate::media::StagedAudio>,
) -> Result<(), HostError> {
    let mut published = Vec::with_capacity(staged.len());
    for audio in staged {
        match audio.publish(commit.directory) {
            Ok(audio) => published.push(audio),
            Err(error) => {
                published
                    .into_iter()
                    .for_each(crate::media::PublishedAudio::withdraw);
                return Err(HostError::Package(error));
            }
        }
    }
    let saved = publish_edit(commit);
    if saved
        .as_ref()
        .is_err_and(|error| !error.durability_unknown())
    {
        published
            .into_iter()
            .for_each(crate::media::PublishedAudio::withdraw);
    }
    saved
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

/// Applies `actions` as one edit group. Imports are staged into `staged`;
/// the caller publishes them only when the group is accepted.
fn apply_transaction(
    session: &mut Session,
    directory: &Path,
    actions: Vec<Value>,
    staged: &mut Vec<crate::media::StagedAudio>,
) -> Result<Vec<Action>, HostError> {
    let mut applied = Vec::with_capacity(actions.len());
    let mut prior = Vec::with_capacity(actions.len());
    let mut failure: Option<HostError> = None;
    let result = session.transaction(|transaction| {
        for (index, action) in actions.iter().enumerate() {
            let mut value = action.clone();
            if let Err(error) = resolve_refs(&mut value, &prior) {
                failure = Some(HostError::Element {
                    index,
                    error: ElementError::Reference(error),
                });
                return Err(ActionError::CorruptJournal);
            }
            let command = match Command::from_value(value) {
                Ok(command) => command,
                Err(error) => {
                    failure = Some(HostError::Element {
                        index,
                        error: ElementError::Parameters(error),
                    });
                    return Err(ActionError::CorruptJournal);
                }
            };
            // Import external media on the document thread, then commit only a
            // durable PCM reference. A failure rolls back the whole edit group
            // and drops every stage, so no media remains (EXT-02).
            let command = match import_command(directory, command, staged) {
                Ok(command) => command,
                Err(error) => {
                    failure = Some(HostError::Element {
                        index,
                        error: ElementError::Package(error),
                    });
                    return Err(ActionError::NotApplied);
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
                        error: ElementError::Action(error.clone()),
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

fn import_command(
    directory: &Path,
    command: Command,
    staged: &mut Vec<crate::media::StagedAudio>,
) -> Result<Command, PackageError> {
    if let Command::ImportAudio { track, start, path } = command {
        let audio = crate::media::stage_audio(directory, Path::new(&path))?;
        let stored = audio.stored().clone();
        staged.push(audio);
        Ok(Command::PlaceAudio {
            track,
            start,
            hash: stored.hash,
            sample_rate: stored.sample_rate,
            channels: stored.channels,
            frames: stored.frames,
        })
    } else {
        Ok(command)
    }
}

fn resolve_refs(value: &mut Value, prior: &[Value]) -> Result<(), ReferenceError> {
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

fn reference(value: &Value, prior: &[Value]) -> Result<Option<Value>, ReferenceError> {
    let Value::Object(map) = value else {
        return Ok(None);
    };
    if !is_reference(map) {
        return Ok(None);
    }
    let Some(index) = map.get("ref").and_then(Value::as_u64) else {
        return Err(ReferenceError::NotIndex);
    };
    let Some(field) = map.get("field").and_then(Value::as_str) else {
        return Err(ReferenceError::MissingField);
    };
    let Some(index) = usize::try_from(index).ok() else {
        return Err(ReferenceError::OutOfRange);
    };
    let Some(earlier) = prior.get(index) else {
        return Err(ReferenceError::OutOfRange);
    };
    let Some(found) = earlier.get(field).and_then(Value::as_str) else {
        return Err(ReferenceError::FieldNotFound);
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
        .map_err(|_| HostError::Package(PackageError::Document(DocumentError::InvalidRevision)))?;
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
                if let Err(error) = compact_journal(directory, seq) {
                    log::warn!(target: LOG, seq = seq, error:% = error, cause:? = error; "the journal was not compacted");
                }
            }
            Some(Err(error)) => {
                log::warn!(target: LOG, seq = *seq, error:% = error, cause:? = error; "an autosave failed");
                inflight.remove(0);
            }
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
