//! One thread owns a document. Callers send commands and read a snapshot.

use std::collections::HashSet;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};

use echode_actions::{Action, Command, HistoryUsage, JournalLine, JournalOp, Session};
use echode_model::{ChangeSet, Id, Project};
use gix::ObjectId;
use serde_json::{Map, Value};

use crate::autosave::{Autosave, PendingAutosave, SavedRevision};
use crate::document::{
    PackageError, append_journal, compact_journal, next_journal_seq, repair_journal,
};
use crate::imports::StagedImports;

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

type WriteCall = Box<dyn FnOnce(Result<&mut Worker, HostError>) + Send>;

enum Request {
    /// A write handled only while the document has not halted.
    Write(WriteCall),
    /// Read-only, so a halted thread still answers it.
    Revisions {
        reply: Sender<Result<Vec<SavedRevision>, HostError>>,
    },
    /// Read-only, so a halted thread still answers it.
    Usage {
        reply: Sender<Result<HistoryUsage, HostError>>,
    },
    #[cfg(test)]
    StopAutosave {
        reply: Sender<()>,
    },
    Shutdown,
}

/// The only owner of a package's [`Session`].
pub struct DocumentHost {
    request: Sender<Request>,
    thread: Option<JoinHandle<()>>,
    snapshot: Arc<SharedDocument>,
    lock: PackageLock,
    /// Commit recovery read, and the last journal line included in that project.
    base_revision: String,
    applied_seq: u64,
    recovery: Option<RecoveryNotice>,
    recovery_pending: Arc<AtomicBool>,
}

/// The worker publishes the project and its terminal write status here.
struct SharedDocument {
    project: Mutex<Arc<Project>>,
    halted: AtomicBool,
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
        let next_seq = next_journal_seq(&directory)?.max(applied_seq.saturating_add(1));
        let snapshot = Arc::new(SharedDocument {
            project: Mutex::new(Arc::new(project.clone())),
            halted: AtomicBool::new(false),
        });
        let autosave = Autosave::create(&directory)?;
        // A confirmed edit is journaled before it returns. The first autosave
        // can still die with `save.pending` and no tip, and then recover
        // refuses the worktree. Commit the recovered project first, so a later
        // kill always has a tip to replay onto. That new commit is the base.
        // Recovery is authoritative. Probing the repository again here would
        // race the autosave worker while it creates the initial Git metadata.
        if base_revision.is_empty() {
            let id = autosave
                .save(Arc::new(project.clone()), applied_seq)?
                .wait()?;
            base_revision = id.to_string();
        }
        let (request, incoming) = mpsc::channel();
        let worker = Worker {
            directory: directory.clone(),
            autosave,
            snapshot: Arc::clone(&snapshot),
            session: Session::open(project),
            recovery_pending: Arc::clone(&recovery_pending),
            inflight: Vec::new(),
            next_seq,
            autosave_status: crate::retry::BackgroundRetry::default(),
        };
        let thread = thread::Builder::new()
            .name("echode-document".to_string())
            .spawn(move || worker.run(incoming))
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
                .project
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
        self.commit_plugin_save(device, Arc::new(crate::PluginState::new(bytes.to_vec())))
    }

    fn call<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, HostError>>) -> Request,
    ) -> Result<T, HostError> {
        let (reply, incoming) = mpsc::channel();
        self.request
            .send(build(reply))
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?;
        incoming
            .recv()
            .map_err(|_| HostError::Package(PackageError::Document(DocumentError::Stopped)))?
    }

    fn write_unconfirmed<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Worker) -> Result<T, HostError> + Send + 'static,
    ) -> Result<T, HostError> {
        self.call(|reply| {
            Request::Write(Box::new(move |worker| {
                // Bind the outcome so any captured StagedImports drop before reply.
                let result = worker.and_then(action);
                let _ = reply.send(result);
            }))
        })
    }

    fn write<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Worker) -> Result<T, HostError> + Send + 'static,
    ) -> Result<T, HostError> {
        self.require_recovery_confirmation()
            .map_err(HostError::Package)?;
        self.write_unconfirmed(action)
    }

    /// Records a capture without rewriting its object after a journal failure.
    pub fn commit_plugin_save(
        &self,
        device: Id,
        capture: Arc<crate::PluginState>,
    ) -> Result<bool, HostError> {
        self.write(move |worker| worker.absorb_plugin_state(device, capture))
    }

    /// No more writes may be attempted until this document is reopened.
    pub fn edits_halted(&self) -> bool {
        self.snapshot.halted.load(Ordering::Acquire)
    }

    /// Applies `command`, writing any nonempty edit to the journal before returning.
    /// A no-op succeeds without publishing or discarding redo.
    pub fn perform(&self, command: Command) -> Result<Action, HostError> {
        self.perform_staged(command, StagedImports::none())
    }

    /// [`Self::perform`] with an import already staged off this thread. The
    /// result, journal and store are the same as staging it here (ADR 0331).
    pub fn perform_staged(
        &self,
        command: Command,
        imports: StagedImports,
    ) -> Result<Action, HostError> {
        self.write(move |worker| worker.perform(command, imports))
    }

    /// Applies every action as one undo step. References are resolved in order.
    /// A group with no net change returns all accepted actions without recording a step.
    pub fn apply(&self, actions: Vec<Value>) -> Result<Vec<Action>, HostError> {
        self.apply_staged(actions, StagedImports::none())
    }

    /// [`Self::apply`] with the group's imports already staged off this thread.
    pub fn apply_staged(
        &self,
        actions: Vec<Value>,
        imports: StagedImports,
    ) -> Result<Vec<Action>, HostError> {
        self.write(move |worker| worker.apply(actions, imports))
    }

    /// Undoes the newest undo step and appends an `undo` journal line.
    pub fn undo(&self) -> Result<(), HostError> {
        self.write(|worker| worker.step_history(true))
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
        self.write_unconfirmed(Worker::accept_recovery)
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
        self.call(|reply| Request::Revisions { reply })
    }

    /// What undo and redo keep in memory beyond the current project (ADR 0286).
    ///
    /// The document thread walks its history, so the time grows with the
    /// number of steps. Call it for diagnosis, not on every frame.
    pub fn history_usage(&self) -> Result<HistoryUsage, HostError> {
        self.call(|reply| Request::Usage { reply })
    }

    /// Redoes the newest undone step and appends a `redo` journal line.
    pub fn redo(&self) -> Result<(), HostError> {
        self.write(|worker| worker.step_history(false))
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

struct Worker {
    directory: std::path::PathBuf,
    autosave: Autosave,
    snapshot: Arc<SharedDocument>,
    session: Session,
    recovery_pending: Arc<AtomicBool>,
    inflight: Vec<(u64, PendingAutosave)>,
    next_seq: u64,
    autosave_status: crate::retry::BackgroundRetry,
}

impl Worker {
    fn halted(&self) -> bool {
        self.snapshot.halted.load(Ordering::Acquire)
    }

    fn run(mut self, incoming: Receiver<Request>) {
        while let Ok(request) = incoming.recv() {
            self.finish_saves();
            match request {
                Request::Shutdown => break,
                #[cfg(test)]
                Request::StopAutosave { reply } => {
                    if !self.halted() {
                        self.autosave.stop_for_test();
                    }
                    let _ = reply.send(());
                }
                Request::Revisions { reply } => {
                    self.wait_for_saves();
                    let _ = reply.send(self.autosave.revisions().map_err(HostError::Package));
                }
                Request::Usage { reply } => {
                    let mut usage = self.session.usage();
                    usage.autosave_stopped = self.autosave_status.stopped();
                    let _ = reply.send(Ok(usage));
                }
                Request::Write(call) => {
                    let outcome = if self.halted() {
                        Err(HostError::Package(PackageError::durability_unknown()))
                    } else {
                        Ok(&mut self)
                    };
                    call(outcome);
                }
            }
        }
        self.wait_for_saves();
        self.log_closed();
    }

    fn accept_recovery(&mut self) -> Result<(), HostError> {
        if !self.recovery_pending.load(Ordering::Acquire) {
            return Ok(());
        }
        let seq = self.next_seq.saturating_sub(1);
        self.autosave
            .save(Arc::new(self.session.project().clone()), seq)
            .and_then(|pending| pending.wait())
            .and_then(|_| compact_journal(&self.directory, seq))
            .map(|()| self.recovery_pending.store(false, Ordering::Release))
            .map_err(HostError::Package)
    }

    fn perform(
        &mut self,
        command: Command,
        mut imports: StagedImports,
    ) -> Result<Action, HostError> {
        let result = match command {
            Command::Restore { revision } => {
                self.wait_for_saves();
                let parked = self.session.park_redo();
                match restore_revision(&mut self.session, &self.autosave, &revision) {
                    Ok(changes) if changes.is_empty() => {
                        self.session.unpark_redo(parked);
                        Ok(Action::Restore { revision })
                    }
                    Ok(_) => self
                        .commit_edit(parked, Vec::new())
                        .map(|()| Action::Restore { revision }),
                    Err(error) => {
                        self.session.unpark_redo(parked);
                        Err(error)
                    }
                }
            }
            Command::ImportAudio { track, start, path } => {
                let staged = imports
                    .take_or_stage(&self.directory, &path)
                    .map_err(HostError::Package)?;
                let stored = staged.stored().clone();
                let parked = self.session.park_redo();
                match self.session.perform(Command::PlaceAudio {
                    track,
                    start,
                    hash: stored.hash,
                    sample_rate: stored.sample_rate,
                    channels: stored.channels,
                    frames: stored.frames,
                }) {
                    Ok(action) => self.commit_edit(parked, vec![staged]).map(|()| action),
                    Err(error) => {
                        self.session.unpark_redo(parked);
                        Err(HostError::Action(error))
                    }
                }
            }
            command => {
                let journal_len = self.session.journal().len();
                let parked = self.session.park_redo();
                match self.session.perform(command) {
                    Ok(action) if self.session.journal().len() == journal_len => {
                        self.session.unpark_redo(parked);
                        Ok(action)
                    }
                    Ok(action) => self.commit_edit(parked, Vec::new()).map(|()| action),
                    Err(error) => {
                        self.session.unpark_redo(parked);
                        Err(HostError::Action(error))
                    }
                }
            }
        };
        // An unused stage is removed before the caller hears back.
        drop(imports);
        result
    }

    fn apply(
        &mut self,
        actions: Vec<Value>,
        mut imports: StagedImports,
    ) -> Result<Vec<Action>, HostError> {
        let journal_len = self.session.journal().len();
        let parked = self.session.park_redo();
        let mut staged = Vec::new();
        let result = match apply_transaction(
            &mut self.session,
            &self.directory,
            actions,
            &mut imports,
            &mut staged,
        ) {
            Ok(applied) if self.session.journal().len() == journal_len => {
                self.session.unpark_redo(parked);
                Ok(applied)
            }
            Ok(applied) => self.commit_edit(parked, staged).map(|()| applied),
            Err(error) => {
                self.session.unpark_redo(parked);
                Err(error)
            }
        };
        // An unused stage is removed before the caller hears back.
        drop(imports);
        result
    }

    fn step_history(&mut self, undo: bool) -> Result<(), HostError> {
        let before = self.session.project().clone();
        let stored = if undo {
            self.session.undo()
        } else {
            self.session.redo()
        }
        .map_err(HostError::Action)?;
        let after = self.session.project().clone();
        let (op, changes) = if undo {
            (
                JournalOp::Undo,
                connecting_changeset(&after, &before, stored),
            )
        } else {
            (
                JournalOp::Redo,
                connecting_changeset(&before, &after, stored),
            )
        };
        let saved = self.publish(op, Vec::new(), changes);
        self.invert_on_failure(saved, undo)
    }

    fn absorb_plugin_state(
        &mut self,
        device: Id,
        capture: Arc<crate::PluginState>,
    ) -> Result<bool, HostError> {
        if device_state(self.session.project(), device).is_none() {
            return Ok(false);
        }
        let state = capture.store(&self.directory).map_err(HostError::Package)?;
        let previous = self.session.project().clone();
        match self
            .session
            .absorb_plugin_state(device, Some(state.clone()))
        {
            Ok(None) => Ok(true),
            Ok(Some(changes)) => {
                let saved = self.publish(
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
                        self.session.reinstall_current(previous);
                        if error.durability_unknown() {
                            note_failure(&self.snapshot.halted, &error);
                        }
                        Err(error)
                    }
                }
            }
            Err(error) => {
                if device_state(self.session.project(), device).is_none() {
                    Ok(false)
                } else {
                    Err(HostError::Action(error))
                }
            }
        }
    }

    fn commit_edit(
        &mut self,
        parked: echode_actions::ParkedRedo,
        staged: Vec<crate::media::StagedAudio>,
    ) -> Result<(), HostError> {
        let saved = self.publish_with_media(staged);
        self.settle(parked, saved)
    }

    /// Keeps a published edit. A failed publish removes that edit and restores the
    /// redo stacks that were parked, without copying the undo history.
    fn settle(
        &mut self,
        parked: echode_actions::ParkedRedo,
        result: Result<(), HostError>,
    ) -> Result<(), HostError> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                note_failure(&self.snapshot.halted, &error);
                let _ = self.session.revert_commit(parked);
                Err(error)
            }
        }
    }

    /// Undo and redo invert each other. `undo` is true when the failed publish
    /// followed `Session::undo`, so `redo` puts that step back.
    fn invert_on_failure(
        &mut self,
        result: Result<(), HostError>,
        undo: bool,
    ) -> Result<(), HostError> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                note_failure(&self.snapshot.halted, &error);
                let restored = if undo {
                    self.session.redo().map(|_| ())
                } else {
                    self.session.undo().map(|_| ())
                };
                if restored.is_err() {
                    self.snapshot.halted.store(true, Ordering::Release);
                }
                Err(error)
            }
        }
    }

    /// Publishes the media an accepted edit imported, then its journal line.
    ///
    /// The objects and their directory entries are synced before the line that
    /// names them (XCA-03). When the line is cleanly not written, objects this
    /// edit created are removed again; when its durability is unknown they stay,
    /// because reopening may replay the line (EXT-02).
    fn publish_with_media(
        &mut self,
        staged: Vec<crate::media::StagedAudio>,
    ) -> Result<(), HostError> {
        for audio in staged {
            audio.publish(&self.directory).map_err(HostError::Package)?;
        }
        let entry = self
            .session
            .journal()
            .last()
            .expect("a committed edit records a journal entry");
        let actions = entry.actions.clone();
        let changes = entry.changes.clone().unwrap_or_default();
        self.publish(JournalOp::Edit, actions, changes)
    }

    fn publish(
        &mut self,
        op: JournalOp,
        actions: Vec<Action>,
        changes: ChangeSet,
    ) -> Result<(), HostError> {
        let line = JournalLine {
            seq: self.next_seq,
            op,
            actions,
            changes,
        };
        append_journal(&self.directory, &line).map_err(HostError::Package)?;
        self.next_seq = self.next_seq.saturating_add(1);
        let published = Arc::new(self.session.project().clone());
        *self
            .snapshot
            .project
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::clone(&published);
        if !self.autosave_status.stopped() {
            match self.autosave.save(published, line.seq) {
                Ok(pending) => self.inflight.push((line.seq, pending)),
                Err(error) => {
                    if self.autosave_status.stop(&error) {
                        log::error!(
                            target: LOG,
                            package = package_name(&self.directory).as_str(),
                            error:% = error,
                            cause:? = error;
                            "autosave stopped accepting snapshots; edits remain in the journal"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn wait_for_saves(&mut self) {
        for (seq, pending) in self.inflight.drain(..) {
            if pending.wait().is_ok() {
                let _ = compact_journal(&self.directory, seq);
            }
        }
    }

    fn finish_saves(&mut self) {
        while let Some((seq, pending)) = self.inflight.first() {
            match pending.try_wait() {
                None => return,
                Some(Ok(_)) => {
                    let seq = *seq;
                    self.inflight.remove(0);
                    if let Err(error) = compact_journal(&self.directory, seq) {
                        log::warn!(target: LOG, seq = seq, error:% = error, cause:? = error; "the journal was not compacted");
                    }
                }
                Some(Err(error)) => {
                    log::warn!(target: LOG, seq = *seq, error:% = error, cause:? = error; "an autosave failed");
                    self.inflight.remove(0);
                }
            }
        }
    }

    fn log_closed(&self) {
        // Read at close, where the walk over every version delays no edit (ADR 0286).
        let usage = self.session.usage();
        log::info!(
            target: LOG,
            package = package_name(&self.directory).as_str(),
            halted = self.halted(),
            autosave_stopped = self.autosave_status.stopped(),
            undo_steps = usage.undo_steps,
            redo_steps = usage.redo_steps,
            version_bytes = usage.versions.bytes,
            version_tracks = usage.versions.tracks,
            version_placements = usage.versions.placements,
            version_contents = usage.versions.contents,
            version_notes = usage.versions.notes,
            journal_changes = usage.journal_changes,
            journal_bytes = usage.journal_bytes;
            "a document closed"
        );
    }
}

/// The log's target for the document thread.
const LOG: &str = "echode_history::host";

/// The package's file name. The log names the package, not its folder path.
fn package_name(directory: &Path) -> String {
    directory
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn note_failure(halted: &AtomicBool, error: &HostError) {
    if error.durability_unknown() {
        halted.store(true, Ordering::Release);
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

/// Applies `actions` as one edit group. Imports are staged into `staged`;
/// the caller publishes them only when the group is accepted.
fn apply_transaction(
    session: &mut Session,
    directory: &Path,
    actions: Vec<Value>,
    imports: &mut StagedImports,
    staged: &mut Vec<crate::media::StagedAudio>,
) -> Result<Vec<Action>, HostError> {
    let mut applied = Vec::with_capacity(actions.len());
    let mut prior = Vec::with_capacity(actions.len());
    session.transaction(|transaction| {
        for (index, action) in actions.iter().enumerate() {
            let mut value = action.clone();
            resolve_refs(&mut value, &prior).map_err(|error| HostError::Element {
                index,
                error: ElementError::Reference(error),
            })?;
            let command = Command::from_value(value).map_err(|error| HostError::Element {
                index,
                error: ElementError::Parameters(error),
            })?;
            // Import external media staged off or on this thread, then commit
            // only a durable PCM reference. A failure rolls back the whole edit
            // group and drops every stage, so no media remains (EXT-02).
            let command = import_command(directory, command, imports, staged).map_err(|error| {
                HostError::Element {
                    index,
                    error: ElementError::Package(error),
                }
            })?;
            let action = transaction
                .perform(command)
                .map_err(|error| HostError::Element {
                    index,
                    error: ElementError::Action(error),
                })?;
            prior.push(serde_json::to_value(&action).expect("a committed action is json"));
            applied.push(action);
        }
        Ok(applied)
    })
}

fn import_command(
    directory: &Path,
    command: Command,
    imports: &mut StagedImports,
    staged: &mut Vec<crate::media::StagedAudio>,
) -> Result<Command, PackageError> {
    if let Command::ImportAudio { track, start, path } = command {
        let audio = imports.take_or_stage(directory, &path)?;
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

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
