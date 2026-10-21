//! A sample audition beside the project output.
//!
//! A browser selection plays the file's own samples once, over whatever the
//! project is doing. Requests do not pass through the document worker: one
//! audition thread decodes, converts and owns its output, so a slow plugin or
//! save never delays the sound and a long file never delays an edit. The
//! audition has its own IOProc on the project's output device, and HAL mixes
//! it with the project's callback. No plugin helper starts for it.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use echode_engine::{AuditionAudio, AuditionControl, AuditionSound, ForbidAlloc, audition_pair};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::Serialize;

use crate::{OutputDevice, hardware};

/// How often an open output frees returned sounds and checks its device.
const SERVICE: Duration = Duration::from_millis(25);
/// An idle output stays open this long, so stepping through a folder does not
/// restart the device for every file. Nothing is heard while it waits.
const KEEP_OPEN: Duration = Duration::from_secs(5);
/// Enumeration attempts while devices keep changing under the listeners.
const ENUMERATION_ATTEMPTS: usize = 3;
/// Recent requests kept for diagnostics. It bounds this record only, not
/// how many requests, files or outputs the audition takes.
const HISTORY: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditionPhase {
    #[default]
    Idle,
    Loading,
    Sounding,
    Ended,
    Stopped,
    Failed,
    /// A newer request came before this one was heard. Only history
    /// entries report it; the newest request is never superseded.
    Superseded,
}

/// What made the UI ask for a sound. It is recorded, never acted on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditionInput {
    /// Not a pointer or key event, or the caller did not say.
    #[default]
    Other,
    Click,
    Key,
}

/// The input behind a request, as the UI measured it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditionOrigin {
    pub input: AuditionInput,
    /// Microseconds from the input event to the request.
    pub delay_us: Option<u64>,
}

/// One request and when each stage finished, to find where a sound waited.
/// Stage times are microseconds after the request, on the host clock.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AuditionEntry {
    pub request: u64,
    /// `None` for a stop.
    pub path: Option<String>,
    pub input: AuditionInput,
    /// From the input event to the request, measured by the UI.
    pub input_delay_us: Option<u64>,
    /// Wall clock, in milliseconds since the Unix epoch, to match an account
    /// of what was heard.
    pub requested_at_ms: u64,
    /// The last phase it reached while it was the newest request. A sound a
    /// newer one replaced stays `sounding`.
    pub phase: AuditionPhase,
    pub error: Option<String>,
    /// The audition thread took the request. Queued older requests are
    /// skipped, so they never have this.
    pub picked_us: Option<u64>,
    /// The whole file was decoded.
    pub decoded_us: Option<u64>,
    pub source_frames: Option<u64>,
    pub source_rate: Option<u32>,
    /// The output was ready. `opened_output` says whether it had to open.
    pub output_us: Option<u64>,
    pub opened_output: bool,
    /// Device-rate planes were ready, after any rate conversion.
    pub converted_us: Option<u64>,
    /// The sound was handed to the callback.
    pub handed_us: Option<u64>,
    /// When the first block with the sound was due at the output.
    pub heard_us: Option<u64>,
    /// Host nanoseconds of the request.
    #[serde(skip)]
    requested: u64,
}

/// The newest request and what became of it, for the snapshot and diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AuditionReport {
    /// No earlier request sounds once this one is made.
    pub request: u64,
    /// The newest request the audition thread has acted on. Older queued
    /// requests are skipped, so this can jump.
    pub handled: u64,
    pub path: Option<String>,
    pub phase: AuditionPhase,
    pub error: Option<String>,
    /// The open audition output's rate. `None` while no output is open.
    pub sample_rate: Option<u32>,
    /// The latest requests, oldest first.
    pub history: VecDeque<AuditionEntry>,
}

impl AuditionReport {
    fn entry(&mut self, request: u64) -> Option<&mut AuditionEntry> {
        self.history
            .iter_mut()
            .rev()
            .find(|entry| entry.request == request)
    }
}

/// Host nanoseconds now, on the clock HAL stamps output times with.
fn host_now() -> u64 {
    // SAFETY: both read the host clock and take no pointers.
    unsafe { AudioConvertHostTimeToNanos(AudioGetCurrentHostTime()) }
}

fn micros_between(start: u64, end: u64) -> u64 {
    end.saturating_sub(start) / 1_000
}

#[derive(Default)]
struct Shared {
    gate: AtomicU64,
    output: Mutex<Option<String>>,
    report: Mutex<AuditionReport>,
}

impl Shared {
    fn current(&self, request: u64) -> bool {
        self.gate.load(Ordering::Acquire) == request
    }

    /// Records the end of a stage of `request` with the time since it was made.
    fn stage(&self, request: u64, record: impl FnOnce(&mut AuditionEntry, u64)) {
        let now = host_now();
        if let Some(entry) = lock(&self.report).entry(request) {
            let elapsed = micros_between(entry.requested, now);
            record(entry, elapsed);
        }
    }

    /// The first block with the sound of `request` was due at `host` nanoseconds.
    fn heard(&self, request: u64, host: u64) {
        if let Some(entry) = lock(&self.report).entry(request)
            && entry.heard_us.is_none()
        {
            entry.heard_us = Some(micros_between(entry.requested, host));
        }
    }

    /// Changes the report only while `request` is still the newest. Its
    /// history entry keeps the phase either way.
    fn update(&self, request: u64, phase: AuditionPhase, error: Option<String>) {
        let mut report = lock(&self.report);
        if let Some(entry) = report.entry(request) {
            entry.phase = phase;
            entry.error = error.clone();
        }
        if report.request == request {
            report.phase = phase;
            report.error = error;
        }
    }

    fn handled(&self, request: u64) {
        let mut report = lock(&self.report);
        report.handled = report.handled.max(request);
    }

    fn set_rate(&self, rate: Option<u32>) {
        lock(&self.report).sample_rate = rate;
    }
}

enum Request {
    Play { request: u64, path: PathBuf },
    Stop { request: u64 },
}

/// A handle to one document's audition thread. Clones share it; the thread
/// closes its output and exits once every handle is dropped.
#[derive(Clone)]
pub struct Audition {
    shared: Arc<Shared>,
    requests: Sender<Request>,
}

impl Audition {
    /// Starts the thread. No device opens until a requested file has decoded.
    pub fn start() -> Result<Self, String> {
        Self::spawn(|| Box::new(HalOutputs::default()))
    }

    pub(crate) fn spawn(
        outputs: impl FnOnce() -> Box<dyn AuditionOutputs> + Send + 'static,
    ) -> Result<Self, String> {
        let (requests, incoming) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("echode-audition".into())
            .spawn(move || run(&worker, &incoming, outputs()))
            .map_err(|error| error.to_string())?;
        Ok(Self { shared, requests })
    }

    /// Replaces any heard or loading sound with the file at `path`, played
    /// once from its first sample. Returns the request number.
    pub fn play(&self, path: PathBuf) -> u64 {
        self.request(Some(path), AuditionOrigin::default())
    }

    /// [`Audition::play`], recording the input that asked for it.
    pub fn play_with(&self, path: PathBuf, origin: AuditionOrigin) -> u64 {
        self.request(Some(path), origin)
    }

    /// Fades out the heard sound and cancels one still loading.
    pub fn stop(&self) -> u64 {
        self.request(None, AuditionOrigin::default())
    }

    /// [`Audition::stop`], recording the input that asked for it.
    pub fn stop_with(&self, origin: AuditionOrigin) -> u64 {
        self.request(None, origin)
    }

    fn request(&self, path: Option<PathBuf>, origin: AuditionOrigin) -> u64 {
        let requested = host_now();
        let requested_at_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        let mut report = lock(&self.shared.report);
        let request = self
            .shared
            .gate
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let shown = path.as_ref().map(|path| path.display().to_string());
        let phase = if path.is_some() {
            AuditionPhase::Loading
        } else {
            AuditionPhase::Stopped
        };
        report.request = request;
        report.path.clone_from(&shown);
        report.phase = phase;
        report.error = None;
        // Nothing older that is still loading can sound now.
        for entry in &mut report.history {
            if entry.phase == AuditionPhase::Loading {
                entry.phase = AuditionPhase::Superseded;
            }
        }
        if report.history.len() >= HISTORY {
            report.history.pop_front();
        }
        report.history.push_back(AuditionEntry {
            request,
            path: shown,
            input: origin.input,
            input_delay_us: origin.delay_us,
            requested_at_ms,
            phase,
            requested,
            ..AuditionEntry::default()
        });
        drop(report);
        let message = match path {
            Some(path) => Request::Play { request, path },
            None => Request::Stop { request },
        };
        // A thread that already exited has nothing left to sound.
        let _ = self.requests.send(message);
        request
    }

    /// The output the project uses: its UID, or `None` for the system output.
    /// The next request opens there.
    pub fn follow_output(&self, uid: Option<String>) {
        *lock(&self.shared.output) = uid;
    }

    pub fn report(&self) -> AuditionReport {
        lock(&self.shared.report).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Where the audition thread plays. Tests replace the HAL with a buffer.
pub(crate) trait AuditionOutputs {
    /// Opens or keeps an output on `uid`, or on the system output. Its rate,
    /// and whether it had to open.
    fn prepare(&mut self, uid: Option<&str>) -> Result<(u32, bool), String>;
    fn control(&mut self) -> Option<&mut AuditionControl>;
    fn is_open(&self) -> bool;
    /// False once the device changed or the callback failed.
    fn healthy(&self) -> bool;
    /// The newest sound the callback started, and the host nanoseconds its
    /// first block was due at the output.
    fn heard(&mut self) -> Option<(u64, u64)>;
    fn close(&mut self);
}

/// Sound starts noted by the callback and not yet read. A full ring drops
/// a note, which only leaves a diagnostic time empty.
const NOTES: usize = 8;

/// The callback's half: each newly adopted sound id and its block's host time.
pub(crate) type StartNotes = Producer<(u64, u64)>;

/// The callback's notes of the sounds it started, in order.
pub(crate) struct HeardNotes {
    notes: Consumer<(u64, u64)>,
    latest: Option<(u64, u64)>,
}

pub(crate) fn heard_notes() -> (StartNotes, HeardNotes) {
    let (producer, notes) = RingBuffer::new(NOTES);
    (
        producer,
        HeardNotes {
            notes,
            latest: None,
        },
    )
}

impl HeardNotes {
    /// The newest sound id and its host time in nanoseconds. A note without
    /// a host time from HAL is skipped.
    pub(crate) fn latest(&mut self) -> Option<(u64, u64)> {
        while let Ok((sound, host)) = self.notes.pop() {
            if host != 0 {
                // SAFETY: converts a host time value and takes no pointers.
                self.latest = Some((sound, unsafe { AudioConvertHostTimeToNanos(host) }));
            }
        }
        self.latest
    }
}

fn run(shared: &Shared, requests: &Receiver<Request>, mut outputs: Box<dyn AuditionOutputs>) {
    let mut sounding: Option<(u64, u64)> = None;
    let mut idle_since: Option<Instant> = None;
    loop {
        let wait = if outputs.is_open() {
            SERVICE
        } else {
            Duration::from_secs(3600)
        };
        let mut next = match requests.recv_timeout(wait) {
            Ok(request) => Some(request),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Only the newest queued request can still sound.
        while let Ok(request) = requests.try_recv() {
            next = Some(request);
        }
        match next {
            Some(Request::Play { request, path }) => {
                shared.stage(request, |entry, at| entry.picked_us = Some(at));
                if let Some(id) = start(shared, outputs.as_mut(), request, &path) {
                    sounding = Some((request, id));
                }
                shared.handled(request);
            }
            Some(Request::Stop { request }) => {
                shared.stage(request, |entry, at| entry.picked_us = Some(at));
                if let Some(control) = outputs.control() {
                    control.stop();
                }
                sounding = None;
                shared.update(request, AuditionPhase::Stopped, None);
                shared.handled(request);
            }
            None => {}
        }
        service(shared, outputs.as_mut(), &mut sounding, &mut idle_since);
    }
    outputs.close();
}

/// Decodes, opens the output and hands the sound over, unless a newer request
/// arrives first. Returns the sound's id in the output.
fn start(
    shared: &Shared,
    outputs: &mut dyn AuditionOutputs,
    request: u64,
    path: &std::path::Path,
) -> Option<u64> {
    if !shared.current(request) {
        return None;
    }
    let fail = |error: String| {
        shared.update(request, AuditionPhase::Failed, Some(error));
        None
    };
    let pcm = match echode_history::decode_audio_file(path, &|| shared.current(request)) {
        Ok(pcm) => pcm,
        Err(_) if !shared.current(request) => return None,
        Err(error) => return fail(error.to_string()),
    };
    shared.stage(request, |entry, at| {
        entry.decoded_us = Some(at);
        entry.source_frames = Some(pcm.frames);
        entry.source_rate = Some(pcm.sample_rate);
    });
    if !shared.current(request) {
        return None;
    }
    let uid = lock(&shared.output).clone();
    let rate = match outputs.prepare(uid.as_deref()) {
        Ok((rate, opened)) => {
            shared.stage(request, |entry, at| {
                entry.output_us = Some(at);
                entry.opened_output = opened;
            });
            rate
        }
        Err(error) => {
            shared.set_rate(None);
            return fail(error);
        }
    };
    shared.set_rate(Some(rate));
    let sound = crate::clips::device_planes(&pcm, rate)
        .and_then(|planes| AuditionSound::new(planes.left, planes.right));
    let sound = match sound {
        Ok(sound) => sound,
        Err(error) => return fail(error.to_string()),
    };
    shared.stage(request, |entry, at| entry.converted_us = Some(at));
    if !shared.current(request) {
        return None;
    }
    let Some(control) = outputs.control() else {
        return fail("no audition output is open".into());
    };
    let id = control.play(sound);
    shared.stage(request, |entry, at| entry.handed_us = Some(at));
    shared.update(request, AuditionPhase::Sounding, None);
    Some(id)
}

fn service(
    shared: &Shared,
    outputs: &mut dyn AuditionOutputs,
    sounding: &mut Option<(u64, u64)>,
    idle_since: &mut Option<Instant>,
) {
    if !outputs.is_open() {
        *idle_since = None;
        return;
    }
    if !outputs.healthy() {
        outputs.close();
        shared.set_rate(None);
        if let Some((request, _)) = sounding.take() {
            shared.update(
                request,
                AuditionPhase::Stopped,
                Some("the output device changed".into()),
            );
        }
        *idle_since = None;
        return;
    }
    if let Some((request, id)) = *sounding
        && let Some((heard, host)) = outputs.heard()
        && heard == id
    {
        shared.heard(request, host);
    }
    let Some(control) = outputs.control() else {
        return;
    };
    control.service();
    if let Some((request, id)) = *sounding
        && control.heard() != Some(id)
        && control.ended() >= id
    {
        shared.update(request, AuditionPhase::Ended, None);
        *sounding = None;
    }
    if control.idle() && sounding.is_none() {
        let since = *idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= KEEP_OPEN {
            outputs.close();
            shared.set_rate(None);
            *idle_since = None;
        }
    } else {
        *idle_since = None;
    }
}

#[derive(Default)]
struct HalOutputs {
    epoch: Arc<AtomicU64>,
    notifications: Option<hardware::Notifications>,
    open: Option<(Option<String>, AuditionStream)>,
}

impl AuditionOutputs for HalOutputs {
    fn prepare(&mut self, uid: Option<&str>) -> Result<(u32, bool), String> {
        if self.healthy()
            && let Some((asked, stream)) = &self.open
            && asked.as_deref() == uid
        {
            return Ok((stream.device.sample_rate, false));
        }
        self.close();
        let mut attempts = 0;
        let (inventory, notifications, observed) = loop {
            let observed = self.epoch.load(Ordering::Acquire);
            let inventory = hardware::inventory()?;
            let notifications = hardware::Notifications::new(&inventory, Arc::clone(&self.epoch))?;
            // Close the gap between enumeration and registration.
            if hardware::inventory()? == inventory {
                break (inventory, notifications, observed);
            }
            attempts += 1;
            if attempts >= ENUMERATION_ATTEMPTS {
                return Err("the output devices kept changing".into());
            }
        };
        let device = crate::session::desired(&inventory, uid)
            .cloned()
            .ok_or("no output device is available")?;
        let stream = AuditionStream::open(device, Arc::clone(&self.epoch), observed)?;
        let rate = stream.device.sample_rate;
        self.notifications = Some(notifications);
        self.open = Some((uid.map(str::to_owned), stream));
        Ok((rate, true))
    }

    fn control(&mut self) -> Option<&mut AuditionControl> {
        self.open.as_mut().map(|(_, stream)| &mut stream.control)
    }

    fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn healthy(&self) -> bool {
        self.open.as_ref().is_some_and(|(_, stream)| {
            stream.observed == self.epoch.load(Ordering::Acquire) && !stream.failed()
        })
    }

    fn heard(&mut self) -> Option<(u64, u64)> {
        self.open
            .as_mut()
            .and_then(|(_, stream)| stream.heard.latest())
    }

    fn close(&mut self) {
        if let Some((_, mut stream)) = self.open.take() {
            stream.close();
        }
        self.notifications = None;
    }
}

struct CallbackState {
    audio: AuditionAudio,
    left: Vec<f32>,
    right: Vec<f32>,
    heard: StartNotes,
    /// The sound last noted as started, or 0.
    noted: u64,
}

pub(crate) struct AuditionCallback {
    state: UnsafeCell<CallbackState>,
    channels: u32,
    epoch: Arc<AtomicU64>,
    observed: u64,
    failed: AtomicBool,
}

// SAFETY: HAL serializes calls to this IOProc, and only the callback touches
// `state`. The audition thread frees it after stopping and destroying the IOProc.
unsafe impl Sync for AuditionCallback {}

impl AuditionCallback {
    pub(crate) fn new(
        audio: AuditionAudio,
        heard: StartNotes,
        frames: usize,
        channels: u32,
        epoch: Arc<AtomicU64>,
        observed: u64,
    ) -> Self {
        Self {
            state: UnsafeCell::new(CallbackState {
                audio,
                left: vec![0.0; frames],
                right: vec![0.0; frames],
                heard,
                noted: 0,
            }),
            channels,
            epoch,
            observed,
            failed: AtomicBool::new(false),
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

struct AuditionStream {
    device: OutputDevice,
    control: AuditionControl,
    heard: HeardNotes,
    callback: Option<Box<AuditionCallback>>,
    proc: AudioDeviceIOProcID,
    observed: u64,
}

impl AuditionStream {
    fn open(device: OutputDevice, epoch: Arc<AtomicU64>, observed: u64) -> Result<Self, String> {
        hardware::validate_streams(&device)?;
        let (control, audio) =
            audition_pair(device.sample_rate).map_err(|error| error.to_string())?;
        let (notes, heard) = heard_notes();
        let mut callback = Box::new(AuditionCallback::new(
            audio,
            notes,
            device.buffer_frames as usize,
            device.channels,
            epoch,
            observed,
        ));
        let mut proc = None;
        let context = (&mut *callback as *mut AuditionCallback).cast();
        // SAFETY: the boxed context has a stable address and out_proc is SDK typed.
        let status = unsafe {
            AudioDeviceCreateIOProcID(device.id, Some(render), context, NonNull::from(&mut proc))
        };
        if status != 0 {
            return Err(format!("could not open the audition output ({status})"));
        }
        let value = Self {
            device,
            control,
            heard,
            callback: Some(callback),
            proc,
            observed,
        };
        let status = unsafe { AudioDeviceStart(value.device.id, value.proc) };
        if status != 0 {
            return Err(format!("could not start the audition output ({status})"));
        }
        Ok(value)
    }

    fn failed(&self) -> bool {
        self.callback
            .as_ref()
            .is_some_and(|callback| callback.failed())
    }

    fn close(&mut self) {
        if let Some(callback) = self.callback.take() {
            let _ = unsafe { AudioDeviceStop(self.device.id, self.proc) };
            let destroyed = unsafe { AudioDeviceDestroyIOProcID(self.device.id, self.proc) };
            if destroyed != 0
                && destroyed != kAudioHardwareBadObjectError
                && destroyed != kAudioHardwareBadDeviceError
            {
                // HAL could still call it. Freeing it would be unsafe.
                Box::leak(callback);
            }
        }
    }
}

impl Drop for AuditionStream {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe extern "C-unwind" fn render(
    _: u32,
    _: NonNull<AudioTimeStamp>,
    _: NonNull<AudioBufferList>,
    _: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    output_time: NonNull<AudioTimeStamp>,
    context: *mut c_void,
) -> i32 {
    let Some(callback) = (unsafe { context.cast::<AuditionCallback>().as_ref() }) else {
        return 0;
    };
    // SAFETY: HAL passes a valid output time for this call.
    let time = unsafe { output_time.as_ref() };
    let host = if time.mFlags.contains(AudioTimeStampFlags::HostTimeValid) {
        time.mHostTime
    } else {
        0
    };
    // SAFETY: HAL supplies the entire flexible buffer array.
    let list = output.as_ptr();
    let count = unsafe { (*list).mNumberBuffers } as usize;
    let pointer = unsafe { std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>() };
    let buffers = unsafe { std::slice::from_raw_parts_mut(pointer, count) };
    unsafe { fill(callback, buffers, host) };
    0
}

/// Also runs with owned test buffers; no HAL call happens here. `host` is
/// the host time this block is due at the output, or 0 when unknown.
///
/// # Safety
/// HAL serializes calls, and each non-null buffer is writable for its size.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
pub(crate) unsafe fn fill(callback: &AuditionCallback, buffers: &mut [AudioBuffer], host: u64) {
    let _guard = ForbidAlloc::enter();
    let (frames, valid) = unsafe { crate::stream::clear_buffers(buffers) };
    // A changed device stays silent until the thread closes this output.
    if callback.epoch.load(Ordering::Acquire) != callback.observed {
        return;
    }
    let Some(frames) = frames.filter(|frames| *frames != 0) else {
        return;
    };
    let state = unsafe { &mut *callback.state.get() };
    let (Some(left), Some(right)) = (state.left.get_mut(..frames), state.right.get_mut(..frames))
    else {
        callback.failed.store(true, Ordering::Release);
        return;
    };
    if !valid {
        callback.failed.store(true, Ordering::Release);
        return;
    }
    left.fill(0.0);
    right.fill(0.0);
    state.audio.mix(left, right);
    // A newly adopted sound started at this block's first frame.
    let started = state.audio.started();
    if started != state.noted {
        state.noted = started;
        let _ = state.heard.push((started, host));
    }
    unsafe { crate::stream::write_planes(buffers, left, right, callback.channels == 1) };
}

#[cfg(test)]
#[path = "audition_tests.rs"]
mod tests;
