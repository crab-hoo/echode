//! A submission returns before the document thread finishes the command.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{CString, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use echode_actions::Command;
use echode_audio_io::{AudioCommand, AudioState, Audition, Session, TransportCancellation};
use echode_draw::{ArrangeIndex, ArrangeView, Element, ElementId, PianoNote, PianoView, Role};
use echode_history::{DocumentHost, PeakColumn, Waveforms};
use echode_model::{Id, Project};

use crate::activity::ActivityHub;
use crate::{PianoNoteElement, TrackTreeInfo, VisibleElement};

#[path = "lane.rs"]
mod lane;
use lane::{DeviceLane, Returned, RoundTrip};

use crate::{Complete, ECHODE_ERR, ECHODE_OK};

/// An open package. The worker, not the caller, runs [`DocumentHost::perform`].
pub struct Document {
    queue: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    position: Arc<Position>,
    cancellation: TransportCancellation,
    activity: Arc<ActivityHub>,
    /// Sample audition requests go straight to their own thread.
    audition: Audition,
}

#[derive(Default)]
struct Position {
    meters: RwLock<BTreeMap<u128, Arc<echode_audio_io::TrackMeter>>>,
    generation: AtomicU32,
    tick: AtomicI64,
    playing: AtomicU32,
}

/// One owned reference to a project snapshot and the arrange index for that version.
pub struct Snapshot {
    project: Arc<Project>,
    pub(crate) controls: Arc<crate::controls::ControlIndex>,
    index: Arc<ArrangeIndex>,
    audio: Arc<Vec<u8>>,
    history: Arc<Vec<u8>>,
    device: Arc<Vec<u8>>,
    waveforms: Arc<Waveforms>,
    hierarchy: Arc<Vec<TrackTreeInfo>>,
    row_map: Option<Arc<Vec<usize>>>,
}

pub(crate) enum Operation {
    Perform(Vec<u8>),
    Audio(Vec<u8>),
    History(Vec<u8>),
    Device(Vec<u8>),
    Snapshot,
    Undo,
    Redo,
    Invalid,
}

struct Context(*mut c_void);

// The pointer is the caller's until the completion returns it. This thread
// does not dereference it.
unsafe impl Send for Context {}

struct Job {
    operation: Operation,
    context: Context,
    complete: Complete,
    transport_revision: u64,
}

/// Runs on the worker once its session exists.
type Configure = Box<dyn FnOnce(&mut Session) + Send>;

impl Document {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        Self::open_with(path, Box::new(|_| {}))
    }

    /// The document with a device-free output whose preparations `output`
    /// can hold. Everything else is the product's.
    #[cfg(test)]
    pub(crate) fn open_headless(
        path: &Path,
        output: echode_audio_io::HeadlessOutput,
    ) -> Result<Self, String> {
        Self::open_with(
            path,
            Box::new(move |session| session.use_headless_output(output)),
        )
    }

    fn open_with(path: &Path, configure: Configure) -> Result<Self, String> {
        let host = DocumentHost::open(path).map_err(|error| error.to_string())?;
        let (sender, incoming) = mpsc::channel();
        let position = Arc::new(Position::default());
        let published = Arc::clone(&position);
        let cancellation = TransportCancellation::default();
        let transport = cancellation.clone();
        let lane = DeviceLane::start()?;
        let activity = Arc::new(ActivityHub::new());
        let hub = Arc::clone(&activity);
        let audition = Audition::start()?;
        let follower = audition.clone();
        let thread = thread::Builder::new()
            .name("echode-ffi".to_string())
            .spawn(move || {
                worker(
                    host, incoming, published, transport, lane, hub, follower, configure,
                )
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            queue: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            position,
            cancellation,
            activity,
            audition,
        })
    }

    /// Plays `path` once over the project, replacing any audition, or fades
    /// the audition out for `None`. Never waits for the document worker.
    /// `origin` is only recorded in the audition's history.
    pub(crate) fn audition(
        &self,
        path: Option<std::path::PathBuf>,
        origin: echode_audio_io::AuditionOrigin,
    ) -> u64 {
        match path {
            Some(path) => self.audition.play_with(path, origin),
            None => self.audition.stop_with(origin),
        }
    }

    /// Plugin activity after `after`, waiting up to `timeout`. Never waits for
    /// the worker.
    pub(crate) fn plugin_activity(&self, after: u64, timeout: Duration) -> Vec<u8> {
        self.activity.read(after, timeout)
    }

    pub(crate) fn track_peak(&self, id: u128) -> f32 {
        self.position
            .meters
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .map_or(0.0, |meter| meter.peak())
    }

    pub(crate) fn position(&self) -> crate::TransportPosition {
        loop {
            let generation = self.position.generation.load(Ordering::SeqCst);
            if !generation.is_multiple_of(2) {
                continue;
            }
            let position = crate::TransportPosition {
                tick: self.position.tick.load(Ordering::SeqCst),
                playing: self.position.playing.load(Ordering::SeqCst),
                pad: 0,
            };
            if generation == self.position.generation.load(Ordering::SeqCst) {
                return position;
            }
        }
    }

    pub(crate) fn shutdown(&self) {
        self.cancellation.request(&AudioCommand::Stop);
        // Cancels a decode in progress. The thread closes its output and exits
        // once the last handle is dropped with this document.
        self.audition.stop();
        {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.take();
        }
        // A completion callback runs on this worker. Joining it here is a
        // self-join and aborts. The owner joins after the callback returns.
        let handle = {
            let mut thread = self
                .thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let on_worker = thread
                .as_ref()
                .is_some_and(|thread| thread.thread().id() == thread::current().id());
            if on_worker {
                return;
            }
            thread.take()
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    pub(crate) fn submit(&self, bytes: &[u8], context: *mut c_void, complete: Complete) {
        self.enqueue(Operation::Perform(bytes.to_vec()), context, complete);
    }

    pub(crate) fn enqueue(&self, operation: Operation, context: *mut c_void, complete: Complete) {
        let mut job = Job {
            operation,
            context: Context(context),
            complete,
            transport_revision: 0,
        };
        let rejected = {
            let queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queue.as_ref() {
                None => Some(job),
                Some(sender) => {
                    // Capture the generation in queue order. Stop silences an
                    // existing callback and invalidates a loading Play here,
                    // without waiting behind that load on the document worker.
                    if let Operation::Audio(bytes) = &job.operation
                        && let Ok(command) = serde_json::from_slice::<AudioCommand>(bytes)
                    {
                        job.transport_revision = self.cancellation.request(&command);
                    }
                    sender.send(job).err().map(|error| error.0)
                }
            }
        };
        if let Some(job) = rejected {
            finish(&job, Err("document thread stopped".to_string()));
        }
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Snapshot {
    pub(crate) fn flicks_at(&self, tick: i64) -> Option<i64> {
        self.project
            .tempo()
            .flicks_at(echode_model::Tick::new(tick))
            .map(|time| time.get())
    }
    pub(crate) fn from_project(project: Arc<Project>, previous: Option<&ArrangeIndex>) -> Self {
        let index = previous.map_or_else(
            || ArrangeIndex::build(&project),
            |index| index.update(&project),
        );
        Self {
            controls: Arc::new(crate::controls::ControlIndex::build(&project)),
            hierarchy: Arc::new(crate::projection::hierarchy(&project)),
            row_map: None,
            project,
            index: Arc::new(index),
            audio: Arc::new(serde_json::to_vec(&AudioState::default()).unwrap_or_default()),
            history: Arc::new(empty_history()),
            device: Arc::new(empty_device()),
            waveforms: Arc::new(Waveforms::empty()),
        }
    }

    pub(crate) fn retain(&self) -> Self {
        Self {
            project: Arc::clone(&self.project),
            controls: Arc::clone(&self.controls),
            index: Arc::clone(&self.index),
            audio: Arc::clone(&self.audio),
            history: Arc::clone(&self.history),
            device: Arc::clone(&self.device),
            waveforms: Arc::clone(&self.waveforms),
            hierarchy: Arc::clone(&self.hierarchy),
            row_map: self.row_map.clone(),
        }
    }

    pub(crate) fn project_arrange(&self, collapsed: &BTreeSet<Id>) -> Self {
        if collapsed.is_empty() {
            let mut projected = self.retain();
            projected.row_map = None;
            return projected;
        }
        let mut hidden = BTreeSet::new();
        let rows =
            self.project
                .tracks()
                .iter()
                .enumerate()
                .filter_map(|(index, track)| {
                    if track.parent.is_some_and(|parent| {
                        collapsed.contains(&parent) || hidden.contains(&parent)
                    }) {
                        hidden.insert(track.id);
                        None
                    } else {
                        Some(index)
                    }
                })
                .collect();
        let mut projected = self.retain();
        projected.row_map = Some(Arc::new(rows));
        projected
    }

    pub(crate) fn source_track_index(&self, row: usize) -> Option<usize> {
        match &self.row_map {
            Some(rows) => rows.get(row).copied(),
            None => (row < self.project.tracks().len()).then_some(row),
        }
    }

    pub(crate) fn track_tree(&self, row: usize) -> Option<TrackTreeInfo> {
        self.hierarchy.get(self.source_track_index(row)?).copied()
    }

    fn track(&self, row: usize) -> Option<&echode_model::Track> {
        self.project
            .tracks()
            .get(self.source_track_index(row)?)
            .map(AsRef::as_ref)
    }

    pub(crate) fn waveform(
        &self,
        placement: Id,
        start_tick: i64,
        end_tick: i64,
        columns: &mut [PeakColumn],
    ) -> usize {
        self.waveforms
            .fill(&self.project, placement, start_tick, end_tick, columns)
    }

    pub(crate) fn audio(&self) -> &[u8] {
        &self.audio
    }

    pub(crate) fn history(&self) -> &[u8] {
        &self.history
    }

    pub(crate) fn device(&self) -> &[u8] {
        &self.device
    }

    /// The same snapshot, reporting a device command's result.
    fn with_device(mut self, report: Vec<u8>) -> Self {
        self.device = Arc::new(report);
        self
    }

    pub(crate) fn track_count(&self) -> usize {
        self.row_map
            .as_ref()
            .map_or(self.project.tracks().len(), |rows| rows.len())
    }

    pub(crate) fn track_name(&self, index: usize) -> Option<&str> {
        self.track(index).map(|track| track.name.as_str())
    }

    pub(crate) fn write_track_id(&self, index: usize, out: &mut [u8; 16]) -> bool {
        let Some(track) = self.track(index) else {
            return false;
        };
        *out = id_bytes(track.id);
        true
    }

    pub(crate) fn device_count(&self, track: usize) -> usize {
        self.track(track)
            .map(|track| track.devices.len())
            .unwrap_or(0)
    }

    pub(crate) fn write_device_id(&self, track: usize, device: usize, out: &mut [u8; 16]) -> bool {
        let Some(device) = self
            .track(track)
            .and_then(|track| track.devices.get(device))
        else {
            return false;
        };
        *out = id_bytes(device.id);
        true
    }

    pub(crate) fn device_bypassed(&self, track: usize, device: usize) -> Option<bool> {
        self.track(track)?
            .devices
            .get(device)
            .map(|device| device.bypassed)
    }

    pub(crate) fn device_offline(&self, track: usize, device: usize) -> Option<bool> {
        self.track(track)?
            .devices
            .get(device)
            .map(|device| device.offline)
    }

    pub(crate) fn device_plugin(&self, track: usize, device: usize) -> Option<&str> {
        self.track(track)
            .and_then(|track| track.devices.get(device))
            .and_then(|device| device.plugin.as_deref())
    }

    pub(crate) fn device_pins(&self, track: usize, device: usize) -> Option<&[u32]> {
        self.track(track)
            .and_then(|track| track.devices.get(device))
            .map(|device| device.pinned.as_slice())
    }

    pub(crate) fn write_visible(
        &self,
        view: ArrangeView,
        out: &mut [VisibleElement],
    ) -> (usize, usize) {
        let mut written = 0usize;
        let mut dropped = 0usize;
        let emit = |element| {
            if written < out.len() {
                out[written] = visible_element(element);
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        };
        if let Some(rows) = &self.row_map {
            self.index
                .visit_visible_rows(&self.project, view, rows, emit);
        } else {
            self.index.visit_visible(&self.project, view, emit);
        }
        (written, dropped)
    }

    pub(crate) fn write_piano(
        &self,
        track: Id,
        view: PianoView,
        out: &mut [PianoNoteElement],
    ) -> (usize, usize) {
        let mut written = 0;
        let mut dropped = 0usize;
        self.index.visit_piano(&self.project, track, view, |note| {
            if written < out.len() {
                out[written] = piano_element(note);
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        });
        (written, dropped)
    }

    pub(crate) fn piano_note(
        &self,
        track: Id,
        placement: Option<Id>,
        note: Id,
    ) -> Option<PianoNoteElement> {
        self.index
            .piano_note(&self.project, track, placement, note)
            .map(piano_element)
    }

    pub(crate) fn piano_neighbor(
        &self,
        track: Id,
        placement: Option<Id>,
        note: Option<Id>,
        backwards: bool,
    ) -> Option<PianoNoteElement> {
        let current = note.map(|note| ElementId {
            role: Role::Note,
            track,
            placement,
            note: Some(note),
        });
        self.index
            .piano_neighbor(&self.project, track, current, backwards)
            .map(piano_element)
    }
}

fn piano_element(note: PianoNote) -> PianoNoteElement {
    PianoNoteElement {
        track_id: id_bytes(note.id.track),
        placement_id: note.id.placement.map(id_bytes).unwrap_or([0; 16]),
        note_id: note.id.note.map(id_bytes).unwrap_or([0; 16]),
        start: note.start.get(),
        length: note.length.get(),
        window_start: note.window_start.get(),
        window_end: note.window_end.get(),
        pitch: u32::from(note.pitch),
        velocity: u32::from(note.velocity),
        x: note.x,
        y: note.y,
        w: note.w,
        h: note.h,
    }
}

fn visible_element(element: Element) -> VisibleElement {
    let role = match element.id.role {
        Role::Track => 0,
        Role::Clip => 1,
        Role::Note => 2,
    };
    VisibleElement {
        role,
        track_id: id_bytes(element.id.track),
        placement_id: element.id.placement.map(id_bytes).unwrap_or([0; 16]),
        note_id: element.id.note.map(id_bytes).unwrap_or([0; 16]),
        has_placement: u8::from(element.id.placement.is_some()),
        has_note: u8::from(element.id.note.is_some()),
        pad: [0; 2],
        x: element.x,
        y: element.y,
        w: element.w,
        h: element.h,
    }
}

fn id_bytes(id: Id) -> [u8; 16] {
    *uuid::Uuid::from_u128(id.raw()).as_bytes()
}

#[allow(clippy::too_many_arguments)]
fn worker(
    host: DocumentHost,
    incoming: Receiver<Job>,
    position: Arc<Position>,
    cancellation: TransportCancellation,
    mut lane: DeviceLane,
    activity: Arc<ActivityHub>,
    audition: Audition,
    configure: Configure,
) {
    let mut accepting = true;
    let mut previous: Option<Snapshot> = None;
    let mut audio =
        Session::with_media_and_cancellation(host.snapshot(), host.directory(), cancellation);
    audio.set_audition(audition);
    configure(&mut audio);
    let mut pending_saves: Vec<(u128, Vec<u8>)> = Vec::new();
    // Devices with values changed since their last commit, by the device view
    // or by the plugin's own window.
    let mut editing = BTreeSet::new();
    // Device requests waiting for the output to open, in submission order.
    // Other jobs keep their turns meanwhile.
    let mut waiting: VecDeque<(Job, crate::device::Waiting)> = VecDeque::new();
    loop {
        if accepting && catch_unwind(AssertUnwindSafe(|| audio.service())).is_err() {
            accepting = false;
        }
        if accepting {
            // Before this turn's saves: a change the app will record as one
            // Undo step must not be journaled first as a background save.
            for (device, plugin) in audio.take_plugin_activity() {
                if crate::device::defers_saves(&plugin) {
                    editing.insert(device);
                }
                activity.publish(device, plugin);
            }
            pending_saves.extend(audio.take_plugin_saves());
            let mut captured = false;
            pending_saves.retain(|(device, bytes)| {
                // The commit that ends this edit supersedes these bytes.
                if editing.contains(device) {
                    return true;
                }
                match host.commit_plugin_state(Id::from_raw(*device), bytes) {
                    Ok(_) => {
                        captured = true;
                        false
                    }
                    Err(_) => true,
                }
            });
            if captured {
                audio.update(host.snapshot());
            }
        }
        {
            let meters = audio.meters();
            let mut published = position
                .meters
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if published.len() != meters.len()
                || meters.iter().any(|meter| {
                    published
                        .get(&meter.track)
                        .is_none_or(|old| !Arc::ptr_eq(old, meter))
                })
            {
                *published = meters
                    .into_iter()
                    .map(|meter| (meter.track, meter))
                    .collect();
            }
        }
        publish_position(&position, &audio);
        // Bytes the lane captured are recorded here, in order with other edits.
        while let Some(Returned { job, device, reply }) = lane.take_returned() {
            if !accepting {
                finish(&job, Err("document thread stopped".to_string()));
                continue;
            }
            let before_doc = host.snapshot();
            let performed = catch_unwind(AssertUnwindSafe(|| {
                let report =
                    crate::device::commit(&host, &mut audio, &mut editing, device, &reply?)?;
                publish(&host, &audio, previous.as_ref(), None, Some(report))
            }));
            match performed {
                Ok(Ok(snapshot)) => {
                    publish_position(&position, &audio);
                    previous = Some(snapshot.retain());
                    finish_raw(&job, ECHODE_OK, None, Box::into_raw(Box::new(snapshot)));
                }
                Ok(Err(error)) => finish(&job, Err(error)),
                Err(_) => {
                    accepting = false;
                    finish(&job, Err("document thread panicked".to_string()));
                }
            }
            if !Arc::ptr_eq(&before_doc, &host.snapshot()) {
                pending_saves.clear();
                editing.clear();
            }
        }
        if !waiting.is_empty() && (!accepting || !audio.opening()) {
            while let Some((job, request)) = waiting.pop_front() {
                if !accepting {
                    finish(&job, Err("document thread stopped".to_string()));
                    continue;
                }
                let performed = catch_unwind(AssertUnwindSafe(|| {
                    let dispatch = request.resume(&host, &audio, &mut editing)?;
                    let snapshot = publish(&host, &audio, previous.as_ref(), None, None)?;
                    Ok::<_, String>((dispatch, snapshot))
                }));
                match performed {
                    Ok(Ok((dispatch, snapshot))) => {
                        previous = Some(snapshot.retain());
                        send_to_lane(&mut lane, job, snapshot, dispatch);
                    }
                    Ok(Err(error)) => finish(&job, Err(error)),
                    Err(_) => {
                        accepting = false;
                        finish(&job, Err("document thread panicked".to_string()));
                    }
                }
            }
        }
        let idle = if lane.waiting() || audio.opening() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(20)
        };
        let job = match incoming.recv_timeout(idle) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if !accepting {
            finish(&job, Err("document thread stopped".to_string()));
            continue;
        }
        let before_doc = host.snapshot();
        let mut dispatched = None;
        let performed = catch_unwind(AssertUnwindSafe(|| {
            let history = match &job.operation {
                Operation::Audio(bytes) => {
                    let command: AudioCommand =
                        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
                    audio.perform_requested(command, job.transport_revision)?;
                    None
                }
                Operation::History(bytes) => {
                    let report = history_report(&host, bytes)?;
                    audio.update(host.snapshot());
                    Some(report)
                }
                Operation::Device(bytes) => {
                    dispatched = Some(crate::device::dispatch(
                        &host,
                        &mut audio,
                        &mut editing,
                        bytes,
                        !waiting.is_empty(),
                    )?);
                    None
                }
                _ => {
                    apply(&host, &job.operation)?;
                    audio.update(host.snapshot());
                    None
                }
            };
            publish(&host, &audio, previous.as_ref(), history, None)
        }));
        match performed {
            Ok(Ok(snapshot)) => {
                publish_position(&position, &audio);
                previous = Some(snapshot.retain());
                match dispatched {
                    None => {
                        let snapshot = Box::into_raw(Box::new(snapshot));
                        finish_raw(&job, ECHODE_OK, None, snapshot);
                    }
                    Some(crate::device::Resolution::Ready(dispatch)) => {
                        send_to_lane(&mut lane, job, snapshot, dispatch);
                    }
                    Some(crate::device::Resolution::Waiting(request)) => {
                        waiting.push_back((job, request));
                    }
                }
            }
            Ok(Err(error)) => finish(&job, Err(error)),
            Err(_) => {
                accepting = false;
                finish(&job, Err("document thread panicked".to_string()));
            }
        }
        if !Arc::ptr_eq(&before_doc, &host.snapshot()) {
            // A save captured for the previous graph must not land on the new one.
            // A commit, Undo or other edit also ends any parameter edit.
            pending_saves.clear();
            editing.clear();
        }
    }
    activity.close();
    for (job, _) in waiting.drain(..) {
        finish(&job, Err("document thread stopped".to_string()));
    }
    // Queued device requests are refused. One already waiting ends when its
    // helper exits with the session below.
    lane.close();
    drop(audio);
    lane.join();
    position
        .meters
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    position.playing.store(0, Ordering::SeqCst);
}

/// Hands a resolved request to the lane. A capture returns to the worker to
/// be recorded; anything else completes from the lane with `snapshot`.
fn send_to_lane(
    lane: &mut DeviceLane,
    job: Job,
    snapshot: Snapshot,
    dispatch: crate::device::Dispatch,
) {
    let crate::device::Dispatch {
        device,
        control,
        request,
    } = dispatch;
    let capture = matches!(request, crate::device::Request::Capture);
    let round_trip: RoundTrip =
        Box::new(move || crate::device::round_trip(control.as_ref(), &request));
    if capture {
        lane.capture(job, device, round_trip);
    } else {
        lane.report(job, snapshot, round_trip);
    }
}

fn publish_position(position: &Position, audio: &Session) {
    let (tick, playing) = audio.position();
    position.generation.fetch_add(1, Ordering::SeqCst);
    position.tick.store(tick, Ordering::SeqCst);
    position.playing.store(u32::from(playing), Ordering::SeqCst);
    position.generation.fetch_add(1, Ordering::SeqCst);
}

/// The snapshot a completion hands out. An unchanged project reuses the
/// previous arrange index and waveforms.
fn publish(
    host: &DocumentHost,
    audio: &Session,
    previous: Option<&Snapshot>,
    history: Option<Vec<u8>>,
    device: Option<Vec<u8>>,
) -> Result<Snapshot, String> {
    let project = host.snapshot();
    let mut snapshot = if let Some(previous) = previous
        && Arc::ptr_eq(&project, &previous.project)
    {
        previous.retain()
    } else {
        let mut snapshot = Snapshot::from_project(
            Arc::clone(&project),
            previous.map(|snapshot| &*snapshot.index),
        );
        snapshot.waveforms = Arc::new(Waveforms::load(
            host.directory(),
            &project,
            previous.map(|snapshot| snapshot.waveforms.as_ref()),
        ));
        snapshot
    };
    snapshot.audio =
        Arc::new(serde_json::to_vec(&audio.state()).map_err(|error| error.to_string())?);
    snapshot.history = Arc::new(history.unwrap_or_else(empty_history));
    snapshot.device = Arc::new(device.unwrap_or_else(empty_device));
    Ok(snapshot)
}

fn apply(host: &DocumentHost, operation: &Operation) -> Result<(), String> {
    match operation {
        Operation::Snapshot => Ok(()),
        Operation::Undo => host.undo().map_err(|error| error.to_string()),
        Operation::Redo => host.redo().map_err(|error| error.to_string()),
        Operation::Invalid => Err("unknown document operation".to_string()),
        Operation::Audio(_) => Err("audio command must run on the session".to_string()),
        Operation::History(_) => Err("history command must run on the session".to_string()),
        Operation::Device(_) => Err("device command must run on the session".to_string()),
        Operation::Perform(bytes) => {
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
            let command =
                serde_json::from_str::<Command>(text).map_err(|error| error.to_string())?;
            host.perform(command)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    }
}

fn empty_history() -> Vec<u8> {
    br#"{"revisions":[],"changed":false}"#.to_vec()
}

fn empty_device() -> Vec<u8> {
    b"{}".to_vec()
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
    }))
    .map_err(|error| error.to_string())
}

fn finish(job: &Job, result: Result<(), String>) {
    match result {
        Ok(()) => finish_raw(job, ECHODE_OK, None, std::ptr::null_mut()),
        Err(message) => finish_raw(job, ECHODE_ERR, Some(message), std::ptr::null_mut()),
    }
}

fn finish_raw(job: &Job, status: i32, message: Option<String>, snapshot: *mut Snapshot) {
    let c_message = message.and_then(|message| CString::new(message).ok());
    let pointer = c_message
        .as_ref()
        .map_or(std::ptr::null(), |message| message.as_ptr());
    (job.complete)(job.context.0, status, pointer, snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_stop_submission_invalidates_queued_play_before_worker_receives_it() {
        extern "C" fn complete(
            _: *mut c_void,
            _: i32,
            _: *const std::ffi::c_char,
            _: *mut Snapshot,
        ) {
        }
        let (sender, incoming) = mpsc::channel();
        let document = Document {
            queue: Mutex::new(Some(sender)),
            thread: Mutex::new(None),
            position: Arc::new(Position::default()),
            cancellation: TransportCancellation::default(),
            activity: Arc::new(ActivityHub::new()),
            audition: Audition::start().unwrap(),
        };
        document.enqueue(
            Operation::Audio(br#"{"type":"transport.play"}"#.to_vec()),
            std::ptr::null_mut(),
            complete,
        );
        document.enqueue(
            Operation::Audio(br#"{"type":"transport.stop"}"#.to_vec()),
            std::ptr::null_mut(),
            complete,
        );
        let play = incoming.try_recv().unwrap();
        let stop = incoming.try_recv().unwrap();
        // No worker has run, yet Stop must already cancel the earlier Play.
        assert!(!document.cancellation.is_current(play.transport_revision));
        assert!(document.cancellation.is_current(stop.transport_revision));
        document.enqueue(
            Operation::Audio(br#"{"type":"transport.play"}"#.to_vec()),
            std::ptr::null_mut(),
            complete,
        );
        let replay = incoming.try_recv().unwrap();
        assert!(document.cancellation.is_current(replay.transport_revision));
    }
}
