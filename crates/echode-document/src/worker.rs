//! The document thread: one turn at a time over the history, the audio session
//! and the plugin saves. A panic in a turn leaves their state unknown, so the
//! worker then refuses new work and still completes what it holds.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use echode_actions::Command;
use echode_audio_io::{AudioCommand, Audition, Session, TransportCancellation};
use echode_history::{DocumentHost, StagedImports};
use echode_model::Id;

use crate::activity::ActivityHub;
use crate::device::{self, Dispatch, Request, Resolution, Waiting};
use crate::error::encode;
use crate::import::{Edit, Stage, Staging, follows_imports};
use crate::lane::{DeviceLane, Returned, RoundTrip};
use crate::live::LiveState;
use crate::saves::PluginSaves;
use crate::service::Configure;
use crate::ticket::Ticket;
use crate::{
    DocumentError, EMPTY_DEVICE_REPORT, EMPTY_HISTORY_REPORT, Operation, Presenter, Publication,
    Rejected,
};

/// One submission, in queue order.
pub(crate) struct Job<V> {
    pub(crate) operation: Operation,
    pub(crate) ticket: Ticket<V>,
    pub(crate) transport_revision: u64,
}

/// Everything the document thread owns from its first turn.
pub(crate) struct Start<P: Presenter> {
    pub(crate) host: DocumentHost,
    pub(crate) presenter: P,
    pub(crate) lane: DeviceLane<P::View>,
    pub(crate) live: Arc<LiveState>,
    pub(crate) activity: Arc<ActivityHub>,
    pub(crate) cancellation: TransportCancellation,
    pub(crate) audition: Audition,
    pub(crate) configure: Configure,
    pub(crate) stage: Stage,
}

/// Runs the document thread until every submitter has gone.
pub(crate) fn run<P: Presenter>(start: Start<P>, incoming: Receiver<Job<P::View>>) {
    let Start {
        host,
        presenter,
        lane,
        live,
        activity,
        cancellation,
        audition,
        configure,
        stage,
    } = start;
    let readers = Readers { live, activity };
    let mut audio =
        Session::with_media_and_cancellation(host.snapshot(), host.directory(), cancellation);
    audio.set_audition(audition);
    configure(&mut audio);
    Worker {
        audio,
        lane,
        readers,
        host,
        presenter,
        saves: PluginSaves::default(),
        waiting: VecDeque::new(),
        stage,
        importing: None,
        held: VecDeque::new(),
        accepting: true,
        publishes_live: true,
    }
    .turns(&incoming);
}

/// What the app reads without the worker. They learn the document ended when
/// this drops, even if the worker dies before its first turn.
struct Readers {
    live: Arc<LiveState>,
    activity: Arc<ActivityHub>,
}

impl Drop for Readers {
    fn drop(&mut self) {
        self.activity.close();
        self.live.clear();
    }
}

struct Worker<P: Presenter> {
    // These drop in this order. Ending the session ends a round trip still
    // waiting on its helper, which lets the lane finish and be joined.
    audio: Session,
    lane: DeviceLane<P::View>,
    readers: Readers,
    host: DocumentHost,
    presenter: P,
    saves: PluginSaves,
    /// Device requests waiting for the output to open, in submission order.
    /// Other jobs keep their turns meanwhile.
    waiting: VecDeque<(Ticket<P::View>, Waiting)>,
    stage: Stage,
    /// An edit whose imports stage on their own thread (ADR 0331).
    importing: Option<Importing<P::View>>,
    /// Jobs that follow the import in submission order.
    held: VecDeque<Job<P::View>>,
    accepting: bool,
    publishes_live: bool,
}

/// An edit waiting for its staged imports. Its ticket completes when it lands.
struct Importing<V> {
    ticket: Ticket<V>,
    edit: Edit,
    staging: Staging,
}

/// How a turn ended for its submitter.
enum Outcome<V> {
    Done(V),
    Device(V, Resolution),
    /// The edit imports files, which stage on another thread first.
    Stage(Edit, Vec<String>),
}

impl<P: Presenter> Drop for Worker<P> {
    fn drop(&mut self) {
        // Stops a staging import and waits for it; its edit and the jobs
        // held behind it report that the document stopped.
        self.importing = None;
        self.held.clear();
        self.readers.activity.close();
        // Queued device requests are refused. One already waiting ends when
        // its helper exits with the session.
        self.waiting.clear();
        self.lane.close();
    }
}

impl<P: Presenter> Worker<P> {
    fn turns(mut self, incoming: &Receiver<Job<P::View>>) {
        loop {
            self.background();
            self.publish_live();
            self.record_returned();
            self.resume_waiting();
            if self.land_import() || self.run_held() {
                continue;
            }
            match incoming.recv_timeout(self.idle()) {
                Ok(job) => self.handle(job),
                Err(RecvTimeoutError::Timeout) => {}
                // Every submission completes, so the import and the jobs
                // held behind it finish before the document closes.
                Err(RecvTimeoutError::Disconnected) if self.importing.is_some() => {
                    std::thread::sleep(self.idle());
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn idle(&self) -> Duration {
        if self.lane.waiting() || self.audio.opening() || self.importing.is_some() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(20)
        }
    }

    /// Runs one step. A panic ends accepting and becomes this step's error.
    fn guarded<T>(
        &mut self,
        step: impl FnOnce(&mut Self) -> Result<T, DocumentError>,
    ) -> Result<T, DocumentError> {
        catch_unwind(AssertUnwindSafe(|| step(self))).unwrap_or_else(|_| {
            // The panic hook keeps the reason in the panic log (ADR 0238).
            log::error!(target: "echode_document::worker", "a document step panicked; the document takes no more edits");
            self.accepting = false;
            Err(DocumentError::Panicked)
        })
    }

    /// A step for a submitter, which may change the document. Helpers holding
    /// a change an edit in progress will record outlive it (ADR 0229).
    fn turn<T>(
        &mut self,
        step: impl FnOnce(&mut Self) -> Result<T, DocumentError>,
    ) -> Result<T, DocumentError> {
        self.audio.keep_live_plugin_edits(self.saves.held());
        let before = self.host.snapshot();
        let result = self.guarded(step);
        let after = self.host.snapshot();
        if !Arc::ptr_eq(&before, &after) {
            self.saves.document_changed(&before, &after);
        }
        result
    }

    fn background(&mut self) {
        if !self.accepting {
            return;
        }
        let _ = self.guarded(|worker| {
            worker.audio.service();
            worker.record_plugin_saves();
            Ok(())
        });
    }

    fn record_plugin_saves(&mut self) {
        for (device, plugin) in self.audio.take_plugin_activity() {
            self.saves.note_activity(device, &plugin);
            self.readers.activity.publish(device, plugin);
        }
        self.saves.add(self.audio.take_plugin_saves());
        if self.importing.is_some() {
            // Recorded after the import, as if it had held the worker.
            return;
        }
        let host = &self.host;
        let recorded = self.saves.record(|device, bytes| {
            host.commit_plugin_state(Id::from_raw(device), bytes)
                .is_ok()
        });
        if recorded {
            self.audio.keep_live_plugin_edits(self.saves.held());
            self.audio.update(self.host.snapshot());
        }
    }

    /// Keeps the playhead and meters moving for a session that refuses new
    /// work. A panic here clears them, since nothing would update them again.
    fn publish_live(&mut self) {
        if !self.publishes_live {
            return;
        }
        let published = self.guarded(|worker| {
            worker.readers.live.set_meters(worker.audio.meters());
            worker.publish_position();
            Ok(())
        });
        if published.is_err() {
            self.publishes_live = false;
            self.readers.live.clear();
        }
    }

    fn publish_position(&self) {
        let (tick, playing) = self.audio.position();
        self.readers.live.set_position(tick, playing);
    }

    /// Bytes the lane captured are recorded here, in order with other edits.
    /// While an import stages they wait, so it lands where it was submitted.
    fn record_returned(&mut self) {
        if self.importing.is_some() {
            return;
        }
        while let Some(Returned {
            ticket,
            device,
            reply,
        }) = self.lane.take_returned()
        {
            if !self.accepting {
                continue;
            }
            let result = self.turn(|worker| {
                let report = device::commit(
                    &worker.host,
                    &mut worker.audio,
                    &mut worker.saves.editing,
                    device,
                    reply,
                )?;
                let view = worker.publish(None, Some(report))?;
                worker.publish_position();
                Ok(view)
            });
            ticket.finish(result);
        }
    }

    fn resume_waiting(&mut self) {
        if self.waiting.is_empty() || (self.accepting && self.audio.opening()) {
            return;
        }
        while let Some((ticket, request)) = self.waiting.pop_front() {
            if !self.accepting {
                continue;
            }
            let resumed = self.guarded(|worker| {
                let dispatch =
                    request.resume(&worker.host, &worker.audio, &mut worker.saves.editing)?;
                let view = worker.publish(None, None)?;
                Ok((dispatch, view))
            });
            match resumed {
                Ok((dispatch, view)) => self.send_to_lane(ticket, view, dispatch),
                Err(error) => ticket.finish(Err(error)),
            }
        }
    }

    fn handle(&mut self, job: Job<P::View>) {
        if !self.accepting {
            return;
        }
        if (self.importing.is_some() || !self.held.is_empty()) && follows_imports(&job.operation) {
            self.held.push_back(job);
            return;
        }
        self.run(job);
    }

    fn run(&mut self, job: Job<P::View>) {
        let Job {
            operation,
            ticket,
            transport_revision,
        } = job;
        let performed = self.turn(|worker| worker.perform(operation, transport_revision));
        match performed {
            Ok(Outcome::Done(view)) => ticket.finish(Ok(view)),
            Ok(Outcome::Device(view, Resolution::Ready(dispatch))) => {
                self.send_to_lane(ticket, view, dispatch);
            }
            Ok(Outcome::Device(_, Resolution::Waiting(request))) => {
                self.waiting.push_back((ticket, request));
            }
            Ok(Outcome::Stage(edit, sources)) => self.stage(ticket, edit, sources),
            Err(error) => ticket.finish(Err(error)),
        }
    }

    /// Starts staging `edit`'s imports. A thread that cannot start leaves the
    /// staging to the document thread, as before ADR 0331.
    fn stage(&mut self, ticket: Ticket<P::View>, edit: Edit, sources: Vec<String>) {
        match Staging::start(&self.stage, self.host.directory().to_path_buf(), sources) {
            Ok(staging) => {
                self.importing = Some(Importing {
                    ticket,
                    edit,
                    staging,
                });
            }
            Err(error) => {
                log::warn!(target: "echode_document::import", "staging imports on the document thread: {error}");
                let result = self.turn(|worker| worker.land(edit, Ok(StagedImports::none())));
                ticket.finish(result);
            }
        }
    }

    /// Applies an edit whose imports have staged. Returns whether it did.
    fn land_import(&mut self) -> bool {
        if !self.accepting {
            self.importing = None;
            self.held.clear();
            return false;
        }
        let Some(staged) = self
            .importing
            .as_ref()
            .and_then(|importing| importing.staging.finished())
        else {
            return false;
        };
        let Some(Importing { ticket, edit, .. }) = self.importing.take() else {
            return false;
        };
        let result = self.turn(|worker| worker.land(edit, staged));
        ticket.finish(result);
        true
    }

    /// Runs the next job held behind an import that has landed.
    fn run_held(&mut self) -> bool {
        if self.importing.is_some() || !self.accepting {
            return false;
        }
        let Some(job) = self.held.pop_front() else {
            return false;
        };
        self.run(job);
        true
    }

    fn land(
        &mut self,
        edit: Edit,
        staged: Result<StagedImports, DocumentError>,
    ) -> Result<P::View, DocumentError> {
        let report = self.edit(edit, staged?)?;
        let view = self.publish(Some(report), None)?;
        self.publish_position();
        Ok(view)
    }

    /// Applies `edit` and reports whether it changed the document.
    fn edit(&mut self, edit: Edit, imports: StagedImports) -> Result<Vec<u8>, DocumentError> {
        let before = self.host.snapshot();
        edit.perform(&self.host, imports)?;
        let after = self.host.snapshot();
        let changed = !Arc::ptr_eq(&before, &after);
        self.audio.update(after);
        encode(&serde_json::json!({
            "revisions": [], "changed": changed
        }))
    }

    fn perform(
        &mut self,
        operation: Operation,
        transport_revision: u64,
    ) -> Result<Outcome<P::View>, DocumentError> {
        let mut resolution = None;
        let history = match operation {
            Operation::Audio(bytes) => {
                self.host.require_recovery_confirmation()?;
                let command: AudioCommand =
                    serde_json::from_slice(&bytes).map_err(DocumentError::Malformed)?;
                self.audio.perform_requested(command, transport_revision)?;
                None
            }
            Operation::History(bytes) => {
                let report = history_report(&self.host, &bytes)?;
                self.audio.update(self.host.snapshot());
                Some(report)
            }
            Operation::Device(bytes) => {
                self.host.require_recovery_confirmation()?;
                resolution = Some(device::dispatch(
                    &self.host,
                    &mut self.audio,
                    &mut self.saves.editing,
                    &bytes,
                    !self.waiting.is_empty(),
                )?);
                None
            }
            Operation::Perform(bytes) => {
                let edit = Edit::parse(&bytes)?;
                let sources = edit.import_sources();
                if !sources.is_empty() {
                    // Refused before decoding, as the history would refuse it.
                    self.host.require_recovery_confirmation()?;
                    return Ok(Outcome::Stage(edit, sources));
                }
                Some(self.edit(edit, StagedImports::none())?)
            }
            Operation::Snapshot => {
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Undo => {
                self.host.undo()?;
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Redo => {
                self.host.redo()?;
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Invalid => return Err(Rejected::UnknownOperation.into()),
        };
        let view = self.publish(history, None)?;
        self.publish_position();
        Ok(match resolution {
            Some(resolution) => Outcome::Device(view, resolution),
            None => Outcome::Done(view),
        })
    }

    /// Hands a resolved request to the lane. A capture returns to the worker
    /// to be recorded; anything else completes from the lane with `view`.
    fn send_to_lane(&mut self, ticket: Ticket<P::View>, view: P::View, dispatch: Dispatch) {
        let Dispatch {
            device,
            control,
            request,
        } = dispatch;
        let capture = matches!(request, Request::Capture);
        let round_trip: RoundTrip =
            Box::new(move || device::round_trip(control.as_ref(), &request));
        if capture {
            self.lane.capture(ticket, device, round_trip);
        } else {
            self.lane.report(ticket, view, round_trip);
        }
    }

    fn publish(
        &mut self,
        history: Option<Vec<u8>>,
        device: Option<Vec<u8>>,
    ) -> Result<P::View, DocumentError> {
        let audio = encode(&self.audio.state())?;
        let history = match history {
            Some(report) => report,
            None => encode(&serde_json::json!({
                "revisions": [], "changed": false, "recovery": self.host.recovery_notice()
            }))?,
        };
        Ok(self.presenter.present(Publication {
            project: self.host.snapshot(),
            directory: self.host.directory(),
            audio,
            history,
            device: device.unwrap_or_else(|| EMPTY_DEVICE_REPORT.to_vec()),
        }))
    }
}

fn history_report(host: &DocumentHost, bytes: &[u8]) -> Result<Vec<u8>, DocumentError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(DocumentError::Malformed)?;
    let object = value.as_object().ok_or(Rejected::UnknownHistoryCommand)?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(Rejected::UnknownHistoryCommand)?;
    let before = host.snapshot();
    match kind {
        "history.list" if object.len() == 1 => {}
        "history.export_settings" if object.len() == 1 || object.len() == 2 => {
            if object.len() == 2 {
                let settings = object
                    .get("settings")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(Rejected::ExportSettings)?;
                host.require_recovery_confirmation()?;
                echode_history::write_export_settings(host.directory(), settings)?;
            }
            return encode(&serde_json::json!({
                "revisions": [], "changed": false,
                "export_settings": echode_history::read_export_settings(host.directory()),
            }));
        }
        "history.export_snapshot" if object.len() == 2 => {
            host.require_recovery_confirmation()?;
            let path = object
                .get("path")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())
                .ok_or(Rejected::ExportPath)?;
            echode_history::freeze_export(host.directory(), &before, std::path::Path::new(path))?;
            return Ok(EMPTY_HISTORY_REPORT.to_vec());
        }
        "history.accept_recovery" if object.len() == 3 => {
            let tip = object
                .get("tip")
                .and_then(serde_json::Value::as_str)
                .ok_or(Rejected::RecoveryTip)?;
            let base = object
                .get("base")
                .and_then(serde_json::Value::as_str)
                .ok_or(Rejected::RecoveryBase)?;
            host.accept_recovery(tip, base)?;
            return Ok(EMPTY_HISTORY_REPORT.to_vec());
        }
        "history.restore" if object.len() == 2 => {
            let revision = object
                .get("revision")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or(Rejected::UnknownHistoryCommand)?;
            host.perform(Command::Restore {
                revision: revision.to_string(),
            })?;
        }
        _ => return Err(Rejected::UnknownHistoryCommand.into()),
    }
    let changed = !Arc::ptr_eq(&before, &host.snapshot());
    let revisions = host.revisions()?;
    encode(&serde_json::json!({
        "revisions": revisions,
        "changed": changed,
        "recovery": host.recovery_notice(),
    }))
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
