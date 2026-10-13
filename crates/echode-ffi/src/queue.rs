//! A submission returns before the document thread finishes the command.

use std::ffi::{CString, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use echode_actions::Command;
use echode_audio_io::{AudioCommand, AudioState, Session};
use echode_draw::{ArrangeIndex, ArrangeView, Element, ElementId, PianoNote, PianoView, Role};
use echode_history::{DocumentHost, PeakColumn, Waveforms};
use echode_model::{Id, Project};

use crate::{PianoNoteElement, VisibleElement};

use crate::{Complete, ECHODE_ERR, ECHODE_OK};

/// An open package. The worker, not the caller, runs [`DocumentHost::perform`].
pub struct Document {
    queue: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    position: Arc<Position>,
}

#[derive(Default)]
struct Position {
    generation: AtomicU32,
    tick: AtomicI64,
    playing: AtomicU32,
}

/// One owned reference to a project snapshot and the arrange index for that version.
pub struct Snapshot {
    project: Arc<Project>,
    index: Arc<ArrangeIndex>,
    audio: Arc<Vec<u8>>,
    history: Arc<Vec<u8>>,
    waveforms: Arc<Waveforms>,
}

pub(crate) enum Operation {
    Perform(Vec<u8>),
    Audio(Vec<u8>),
    History(Vec<u8>),
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
}

impl Document {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let host = DocumentHost::open(path).map_err(|error| error.to_string())?;
        let (sender, incoming) = mpsc::channel();
        let position = Arc::new(Position::default());
        let published = Arc::clone(&position);
        let thread = thread::Builder::new()
            .name("echode-ffi".to_string())
            .spawn(move || worker(host, incoming, published))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            queue: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            position,
        })
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
        {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.take();
        }
        let mut thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(thread) = thread.take() {
            let _ = thread.join();
        }
    }

    pub(crate) fn submit(&self, bytes: &[u8], context: *mut c_void, complete: Complete) {
        self.enqueue(Operation::Perform(bytes.to_vec()), context, complete);
    }

    pub(crate) fn enqueue(&self, operation: Operation, context: *mut c_void, complete: Complete) {
        let job = Job {
            operation,
            context: Context(context),
            complete,
        };
        let rejected = {
            let queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queue.as_ref() {
                None => Some(job),
                Some(sender) => sender.send(job).err().map(|error| error.0),
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
    pub(crate) fn from_project(project: Arc<Project>, previous: Option<&ArrangeIndex>) -> Self {
        let index = previous.map_or_else(
            || ArrangeIndex::build(&project),
            |index| index.update(&project),
        );
        Self {
            project,
            index: Arc::new(index),
            audio: Arc::new(serde_json::to_vec(&AudioState::default()).unwrap_or_default()),
            history: Arc::new(empty_history()),
            waveforms: Arc::new(Waveforms::empty()),
        }
    }

    pub(crate) fn retain(&self) -> Self {
        Self {
            project: Arc::clone(&self.project),
            index: Arc::clone(&self.index),
            audio: Arc::clone(&self.audio),
            history: Arc::clone(&self.history),
            waveforms: Arc::clone(&self.waveforms),
        }
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

    pub(crate) fn track_count(&self) -> usize {
        self.project.tracks().len()
    }

    pub(crate) fn track_name(&self, index: usize) -> Option<&str> {
        self.project
            .tracks()
            .get(index)
            .map(|track| track.name.as_str())
    }

    pub(crate) fn write_track_id(&self, index: usize, out: &mut [u8; 16]) -> bool {
        let Some(track) = self.project.tracks().get(index) else {
            return false;
        };
        *out = id_bytes(track.id);
        true
    }

    pub(crate) fn device_count(&self, track: usize) -> usize {
        self.project
            .tracks()
            .get(track)
            .map(|track| track.devices.len())
            .unwrap_or(0)
    }

    pub(crate) fn write_device_id(&self, track: usize, device: usize, out: &mut [u8; 16]) -> bool {
        let Some(device) = self
            .project
            .tracks()
            .get(track)
            .and_then(|track| track.devices.get(device))
        else {
            return false;
        };
        *out = id_bytes(device.id);
        true
    }

    pub(crate) fn device_plugin(&self, track: usize, device: usize) -> Option<&str> {
        self.project
            .tracks()
            .get(track)
            .and_then(|track| track.devices.get(device))
            .and_then(|device| device.plugin.as_deref())
    }

    pub(crate) fn write_visible(
        &self,
        view: ArrangeView,
        out: &mut [VisibleElement],
    ) -> (usize, usize) {
        let mut written = 0usize;
        let mut dropped = 0usize;
        self.index.visit_visible(&self.project, view, |element| {
            if written < out.len() {
                out[written] = visible_element(element);
                written += 1;
            } else {
                dropped = dropped.saturating_add(1);
            }
        });
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

fn worker(host: DocumentHost, incoming: Receiver<Job>, position: Arc<Position>) {
    let mut accepting = true;
    let mut previous: Option<Snapshot> = None;
    let mut audio = Session::with_media(host.snapshot(), host.directory());
    loop {
        if accepting && catch_unwind(AssertUnwindSafe(|| audio.service())).is_err() {
            accepting = false;
        }
        let (tick, playing) = audio.position();
        position.generation.fetch_add(1, Ordering::SeqCst);
        position.tick.store(tick, Ordering::SeqCst);
        position.playing.store(u32::from(playing), Ordering::SeqCst);
        position.generation.fetch_add(1, Ordering::SeqCst);
        let job = match incoming.recv_timeout(Duration::from_millis(20)) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if !accepting {
            finish(&job, Err("document thread stopped".to_string()));
            continue;
        }
        let performed = catch_unwind(AssertUnwindSafe(|| {
            let history = match &job.operation {
                Operation::Audio(bytes) => {
                    let command: AudioCommand =
                        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
                    audio.perform(command)?;
                    None
                }
                Operation::History(bytes) => {
                    let report = history_report(&host, bytes)?;
                    audio.update(host.snapshot());
                    Some(report)
                }
                _ => {
                    apply(&host, &job.operation)?;
                    audio.update(host.snapshot());
                    None
                }
            };
            let project = host.snapshot();
            let mut snapshot = if let Some(previous) = &previous
                && Arc::ptr_eq(&project, &previous.project)
            {
                previous.retain()
            } else {
                let mut snapshot = Snapshot::from_project(
                    Arc::clone(&project),
                    previous.as_ref().map(|snapshot| &*snapshot.index),
                );
                snapshot.waveforms = Arc::new(Waveforms::load(
                    host.directory(),
                    &project,
                    previous
                        .as_ref()
                        .map(|snapshot| snapshot.waveforms.as_ref()),
                ));
                snapshot
            };
            snapshot.audio =
                Arc::new(serde_json::to_vec(&audio.state()).map_err(|error| error.to_string())?);
            snapshot.history = Arc::new(history.unwrap_or_else(empty_history));
            Ok::<_, String>(snapshot)
        }));
        match performed {
            Ok(Ok(snapshot)) => {
                let (tick, playing) = audio.position();
                position.generation.fetch_add(1, Ordering::SeqCst);
                position.tick.store(tick, Ordering::SeqCst);
                position.playing.store(u32::from(playing), Ordering::SeqCst);
                position.generation.fetch_add(1, Ordering::SeqCst);
                previous = Some(snapshot.retain());
                let snapshot = Box::into_raw(Box::new(snapshot));
                finish_raw(&job, ECHODE_OK, None, snapshot);
            }
            Ok(Err(error)) => finish(&job, Err(error)),
            Err(_) => {
                accepting = false;
                finish(&job, Err("document thread panicked".to_string()));
            }
        }
    }
    drop(audio);
    position.playing.store(0, Ordering::SeqCst);
}

fn apply(host: &DocumentHost, operation: &Operation) -> Result<(), String> {
    match operation {
        Operation::Snapshot => Ok(()),
        Operation::Undo => host.undo().map_err(|error| error.to_string()),
        Operation::Redo => host.redo().map_err(|error| error.to_string()),
        Operation::Invalid => Err("unknown document operation".to_string()),
        Operation::Audio(_) => Err("audio command must run on the session".to_string()),
        Operation::History(_) => Err("history command must run on the session".to_string()),
        Operation::Perform(bytes) => {
            let text = match str::from_utf8(bytes) {
                Ok(text) if !text.is_empty() => text,
                Ok(_) => return Err("command is empty".to_string()),
                Err(_) => return Err("command is not utf-8".to_string()),
            };
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
