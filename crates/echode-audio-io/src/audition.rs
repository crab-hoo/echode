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
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use echode_engine::{AuditionAudio, AuditionControl, AuditionSound, ForbidAlloc, audition_pair};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use serde::Serialize;

use crate::{OutputDevice, hardware};

/// How often an open output frees returned sounds and checks its device.
const SERVICE: Duration = Duration::from_millis(25);
/// An idle output stays open this long, so stepping through a folder does not
/// restart the device for every file. Nothing is heard while it waits.
const KEEP_OPEN: Duration = Duration::from_secs(5);
/// Enumeration attempts while devices keep changing under the listeners.
const ENUMERATION_ATTEMPTS: usize = 3;

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

    /// Changes the report only while `request` is still the newest.
    fn update(&self, request: u64, phase: AuditionPhase, error: Option<String>) {
        let mut report = lock(&self.report);
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
        self.request(Some(path))
    }

    /// Fades out the heard sound and cancels one still loading.
    pub fn stop(&self) -> u64 {
        self.request(None)
    }

    fn request(&self, path: Option<PathBuf>) -> u64 {
        let mut report = lock(&self.shared.report);
        let request = self
            .shared
            .gate
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        report.request = request;
        report.path = path.as_ref().map(|path| path.display().to_string());
        report.phase = if path.is_some() {
            AuditionPhase::Loading
        } else {
            AuditionPhase::Stopped
        };
        report.error = None;
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
    /// Opens or keeps an output on `uid`, or on the system output. Its rate.
    fn prepare(&mut self, uid: Option<&str>) -> Result<u32, String>;
    fn control(&mut self) -> Option<&mut AuditionControl>;
    fn is_open(&self) -> bool;
    /// False once the device changed or the callback failed.
    fn healthy(&self) -> bool;
    fn close(&mut self);
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
                if let Some(id) = start(shared, outputs.as_mut(), request, &path) {
                    sounding = Some((request, id));
                }
                shared.handled(request);
            }
            Some(Request::Stop { request }) => {
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
    if !shared.current(request) {
        return None;
    }
    let uid = lock(&shared.output).clone();
    let rate = match outputs.prepare(uid.as_deref()) {
        Ok(rate) => rate,
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
    if !shared.current(request) {
        return None;
    }
    let Some(control) = outputs.control() else {
        return fail("no audition output is open".into());
    };
    let id = control.play(sound);
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
    fn prepare(&mut self, uid: Option<&str>) -> Result<u32, String> {
        if self.healthy()
            && let Some((asked, stream)) = &self.open
            && asked.as_deref() == uid
        {
            return Ok(stream.device.sample_rate);
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
        Ok(rate)
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
    callback: Option<Box<AuditionCallback>>,
    proc: AudioDeviceIOProcID,
    observed: u64,
}

impl AuditionStream {
    fn open(device: OutputDevice, epoch: Arc<AtomicU64>, observed: u64) -> Result<Self, String> {
        hardware::validate_streams(&device)?;
        let (control, audio) =
            audition_pair(device.sample_rate).map_err(|error| error.to_string())?;
        let mut callback = Box::new(AuditionCallback::new(
            audio,
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
    _: NonNull<AudioTimeStamp>,
    context: *mut c_void,
) -> i32 {
    let Some(callback) = (unsafe { context.cast::<AuditionCallback>().as_ref() }) else {
        return 0;
    };
    // SAFETY: HAL supplies the entire flexible buffer array.
    let list = output.as_ptr();
    let count = unsafe { (*list).mNumberBuffers } as usize;
    let pointer = unsafe { std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>() };
    let buffers = unsafe { std::slice::from_raw_parts_mut(pointer, count) };
    unsafe { fill(callback, buffers) };
    0
}

/// Also runs with owned test buffers; no HAL call happens here.
///
/// # Safety
/// HAL serializes calls, and each non-null buffer is writable for its size.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
pub(crate) unsafe fn fill(callback: &AuditionCallback, buffers: &mut [AudioBuffer]) {
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
    unsafe { crate::stream::write_planes(buffers, left, right, callback.channels == 1) };
}

#[cfg(test)]
#[path = "audition_tests.rs"]
mod tests;
