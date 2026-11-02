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
use echode_history::DocumentHost;
use echode_model::Id;

use crate::activity::ActivityHub;
use crate::device::{self, Dispatch, Request, Resolution, Waiting};
use crate::lane::{DeviceLane, Returned, RoundTrip};
use crate::live::LiveState;
use crate::saves::PluginSaves;
use crate::service::Configure;
use crate::ticket::Ticket;
use crate::{EMPTY_DEVICE_REPORT, EMPTY_HISTORY_REPORT, Operation, Presenter, Publication};

const PANICKED: &str = "document thread panicked";

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
    accepting: bool,
    publishes_live: bool,
}

impl<P: Presenter> Drop for Worker<P> {
    fn drop(&mut self) {
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
            match incoming.recv_timeout(self.idle()) {
                Ok(job) => self.handle(job),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn idle(&self) -> Duration {
        if self.lane.waiting() || self.audio.opening() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(20)
        }
    }

    /// Runs one step. A panic ends accepting and becomes this step's error.
    fn guarded<T>(
        &mut self,
        step: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        catch_unwind(AssertUnwindSafe(|| step(self))).unwrap_or_else(|_| {
            self.accepting = false;
            Err(PANICKED.to_string())
        })
    }

    /// A step for a submitter, which may change the document. Helpers holding
    /// a change an edit in progress will record outlive it (ADR 0229).
    fn turn<T>(&mut self, step: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
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
    fn record_returned(&mut self) {
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
                    &reply?,
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
        let Job {
            operation,
            ticket,
            transport_revision,
        } = job;
        if !self.accepting {
            return;
        }
        let performed = self.turn(|worker| worker.perform(operation, transport_revision));
        match performed {
            Ok((view, None)) => ticket.finish(Ok(view)),
            Ok((view, Some(Resolution::Ready(dispatch)))) => {
                self.send_to_lane(ticket, view, dispatch);
            }
            Ok((_, Some(Resolution::Waiting(request)))) => {
                self.waiting.push_back((ticket, request));
            }
            Err(error) => ticket.finish(Err(error)),
        }
    }

    fn perform(
        &mut self,
        operation: Operation,
        transport_revision: u64,
    ) -> Result<(P::View, Option<Resolution>), String> {
        let mut resolution = None;
        let history = match operation {
            Operation::Audio(bytes) => {
                self.host
                    .require_recovery_confirmation()
                    .map_err(|error| error.to_string())?;
                let command: AudioCommand =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                self.audio.perform_requested(command, transport_revision)?;
                None
            }
            Operation::History(bytes) => {
                let report = history_report(&self.host, &bytes)?;
                self.audio.update(self.host.snapshot());
                Some(report)
            }
            Operation::Device(bytes) => {
                self.host
                    .require_recovery_confirmation()
                    .map_err(|error| error.to_string())?;
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
                let before = self.host.snapshot();
                apply_edit(&self.host, &bytes)?;
                let after = self.host.snapshot();
                let changed = !Arc::ptr_eq(&before, &after);
                self.audio.update(after);
                Some(
                    serde_json::to_vec(&serde_json::json!({
                        "revisions": [], "changed": changed
                    }))
                    .map_err(|error| error.to_string())?,
                )
            }
            Operation::Snapshot => {
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Undo => {
                self.host.undo().map_err(|error| error.to_string())?;
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Redo => {
                self.host.redo().map_err(|error| error.to_string())?;
                self.audio.update(self.host.snapshot());
                None
            }
            Operation::Invalid => return Err("unknown document operation".to_string()),
        };
        let view = self.publish(history, None)?;
        self.publish_position();
        Ok((view, resolution))
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
    ) -> Result<P::View, String> {
        let audio = serde_json::to_vec(&self.audio.state()).map_err(|error| error.to_string())?;
        let history = match history {
            Some(report) => report,
            None => serde_json::to_vec(&serde_json::json!({
                "revisions": [], "changed": false, "recovery": self.host.recovery_notice()
            }))
            .map_err(|error| error.to_string())?,
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

fn apply_edit(host: &DocumentHost, bytes: &[u8]) -> Result<(), String> {
    let text = match str::from_utf8(bytes) {
        Ok(text) if !text.is_empty() => text,
        Ok(_) => return Err("command is empty".to_string()),
        Err(_) => return Err("command is not utf-8".to_string()),
    };
    if text.trim_start().starts_with('[') {
        let actions = serde_json::from_str(text).map_err(|error| error.to_string())?;
        return host
            .apply(actions)
            .map(|_| ())
            .map_err(|error| error.to_string());
    }
    let command = serde_json::from_str::<Command>(text).map_err(|error| error.to_string())?;
    host.perform(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn history_report(host: &DocumentHost, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "history command was not recognized".to_string())?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "history command was not recognized".to_string())?;
    let before = host.snapshot();
    match kind {
        "history.list" if object.len() == 1 => {}
        "history.accept_recovery" if object.len() == 3 => {
            let tip = object
                .get("tip")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "recovery tip is missing".to_string())?;
            let base = object
                .get("base")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "recovery base is missing".to_string())?;
            host.accept_recovery(tip, base)
                .map_err(|error| error.to_string())?;
            return Ok(EMPTY_HISTORY_REPORT.to_vec());
        }
        "history.restore" if object.len() == 2 => {
            let revision = object
                .get("revision")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| "history command was not recognized".to_string())?;
            host.perform(Command::Restore {
                revision: revision.to_string(),
            })
            .map_err(|error| error.to_string())?;
        }
        _ => return Err("history command was not recognized".to_string()),
    }
    let changed = !Arc::ptr_eq(&before, &host.snapshot());
    let revisions = host.revisions().map_err(|error| error.to_string())?;
    serde_json::to_vec(&serde_json::json!({
        "revisions": revisions,
        "changed": changed,
        "recovery": host.recovery_notice(),
    }))
    .map_err(|error| error.to_string())
}
