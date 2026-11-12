//! Parent side: spawn `echode-plugin-host` and exchange blocks with it.
//!
//! The mapping is a shared-memory object whose name is unlinked immediately.
//! The child receives that descriptor and a control socket. The audio exchange
//! never touches the socket. A `kqueue` on the control side notices when the
//! helper exits.

use crate::applied::AppliedState;
use crate::control::{ControlMessage, Socket};
use crate::error::{ProtocolError, SessionError, WorkgroupError};
use crate::gui::PluginActivity;
use crate::refusal::Refusal;
use crate::region::{Collected, Delivered, FanIn, MappedRegion, RegionError, SessionLayout};
use crate::sync::{SyncFlags, wake_one};
use crate::workgroup::spawn_helper;
use crate::{CMD_PROCESS, CMD_RESET_PROCESS, CMD_SHUTDOWN};
use echode_realtime::Event;
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const STDERR_CAP: usize = 64 * 1024;

#[derive(Debug)]
pub struct SpawnOptions<'a> {
    pub host_bin: &'a Path,
    pub plugin: Option<&'a Path>,
    /// Passed through as `ECHODE_TEST_FAULT` (`crash` or `hang`).
    pub fault: Option<&'a str>,
    /// When set, the helper is spawned with this workgroup send right.
    pub workgroup: Option<WorkgroupHandoff>,
    pub layout: SessionLayout,
    /// Shared with every helper submitted in the same block. `None` keeps the
    /// single-helper wait on that helper's own notice.
    pub fan_in: Option<Arc<FanIn>>,
    /// Device rate the helper activates at. Zero keeps the helper's 48 kHz default.
    pub sample_rate: u32,
    /// CLAP id inside the bundle. `None` loads the first descriptor.
    pub plugin_id: Option<&'a str>,
}

/// A send right from `os_workgroup_copy_port` and the device period, in mach ticks.
#[derive(Debug)]
pub struct WorkgroupHandoff {
    pub port: u32,
    pub period_ticks: u32,
}

impl Drop for WorkgroupHandoff {
    fn drop(&mut self) {
        crate::workgroup::deallocate_port(self.port);
    }
}

/// What the helper tells the parent without a reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginNotice {
    Dirty,
    PluginRestarted { latency_samples: u32 },
    RequestProcess,
}

struct HelperProcess {
    pid: i32,
    status: Option<ExitStatus>,
}

struct Launched {
    child: HelperProcess,
    stderr: Box<dyn Read + Send>,
    control: Socket,
}

struct WatchSlots {
    notice: *const AtomicU64,
    dead: *const AtomicU64,
    fan: *const FanIn,
}

// SAFETY: the pointers address atomics in a mapping this process keeps alive
// while the watcher holds `WatchSlots`, and the mutex serializes that use.
unsafe impl Send for WatchSlots {}

pub struct PluginHost {
    workgroup: Option<crate::RetainedWorkgroup>,
    region: Arc<MappedRegion>,
    root: Arc<MappedRegion>,
    previous_region: Option<Arc<MappedRegion>>,
    child: HelperProcess,
    control: PluginControl,
    stderr: Arc<Mutex<String>>,
    closed: bool,
    reset_pending: bool,
    host_bin: PathBuf,
    plugin: Option<PathBuf>,
    plugin_id: Option<String>,
    sample_rate: u32,
    watch: Option<JoinHandle<()>>,
    watch_slots: Arc<Mutex<WatchSlots>>,
    watch_stop: Arc<AtomicBool>,
    fan: Option<Arc<FanIn>>,
    /// Channel count of each declared input port. Empty when no plugin is loaded.
    input_ports: Vec<u32>,
    /// Channel count of each declared output port.
    output_ports: Vec<u32>,
    tail_samples: u32,
}

/// Covers the helper's flush and main-thread read bounds. Late replies are
/// still matched by request id, independently of this deadline.
const PARAMETER_REPLY: Duration = Duration::from_secs(5);

/// Longer than the helper's 8 s bound for creating a plugin's view, which can
/// load large resources the first time.
const WINDOW_REPLY: Duration = Duration::from_secs(10);

/// Saving or loading a plugin's state, and the helper's mapping and chain
/// changes. A state moves at about 1 GiB/s (ADR 0318), so states past about
/// 2 GiB do not cross a synchronous request in time.
const STATE_REPLY: Duration = Duration::from_secs(2);

/// Socket and saved-state access for control threads, independent of the
/// realtime mapping. Never call these methods from an audio callback.
///
/// The bytes last applied are kept apart from the socket. Reading them, and
/// polling a background save, never wait behind a request on another thread
/// that is still waiting for the helper's reply.
///
/// Window notices and parameter changes the plugin made on its own are kept
/// apart in the same way. Whoever reads the socket moves them aside, and
/// [`PluginControl::take_activity`] collects them without the socket.
#[derive(Clone)]
pub struct PluginControl {
    socket: Arc<Mutex<ControlState>>,
    applied: Arc<Mutex<Option<AppliedState>>>,
    activity: Arc<Mutex<PluginActivity>>,
    latency_samples: Arc<AtomicU32>,
}

struct ControlState {
    control: Socket,
    inbox: VecDeque<(u64, ControlMessage)>,
    next_request: u64,
    remembered_request: u64,
    pending: HashSet<u64>,
    /// The same storage as [`PluginControl::applied_state`].
    applied: Arc<Mutex<Option<AppliedState>>>,
    /// The same storage as [`PluginControl::take_activity`].
    activity: Arc<Mutex<PluginActivity>>,
    /// The helper showed the window and has not reported it closed.
    window_open: bool,
    dirty_again: bool,
    save_requested: Option<u64>,
    latency_samples: Arc<AtomicU32>,
}

impl PluginControl {
    fn new(control: Socket) -> Self {
        let applied = Arc::new(Mutex::new(None));
        let activity = Arc::new(Mutex::new(PluginActivity::default()));
        let latency_samples = Arc::new(AtomicU32::new(0));
        Self {
            socket: Arc::new(Mutex::new(ControlState {
                control,
                inbox: VecDeque::new(),
                next_request: 0,
                remembered_request: 0,
                pending: HashSet::new(),
                applied: Arc::clone(&applied),
                activity: Arc::clone(&activity),
                window_open: false,
                dirty_again: false,
                save_requested: None,
                latency_samples: Arc::clone(&latency_samples),
            })),
            applied,
            activity,
            latency_samples,
        }
    }

    /// Last latency declared by the live helper, in device-rate samples.
    /// This is metadata only; the engine does not compensate it yet.
    pub fn latency_samples(&self) -> u32 {
        self.latency_samples.load(Ordering::Acquire)
    }

    /// Window notices and plugin-made changes since the last call, in order.
    /// Never waits for the socket. `None` when nothing happened.
    pub fn take_activity(&self) -> Option<PluginActivity> {
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if activity.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut *activity))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ControlState> {
        self.socket
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// What the helper last saved or loaded. Cloning shares the bytes.
    pub fn applied_state(&self) -> Option<AppliedState> {
        self.applied
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Poll notices and collect an asynchronous state save. A newer dirty
    /// notice stays pending until the previous reply has been collected.
    /// A request in flight on another thread keeps the socket; this poll
    /// returns `None` and the save is collected on a later call.
    pub fn poll_state_save(&mut self) -> Option<Vec<u8>> {
        let mut state = match self.socket.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if matches!(state.poll_notice(), Ok(Some(PluginNotice::Dirty))) {
            state.dirty_again = true;
        }
        if state.dirty_again && state.save_requested.is_none() && state.request_save().is_ok() {
            state.dirty_again = false;
        }
        state.save_requested?;
        match state.poll_saved() {
            Ok(Some(Ok(bytes))) => {
                state.save_requested = None;
                Some(bytes)
            }
            Ok(Some(Err(_))) | Err(_) => {
                state.save_requested = None;
                state.dirty_again = false;
                None
            }
            Ok(None) => None,
        }
    }
}

impl ControlState {
    /// Lifecycle notices share the activity stream's order, not the ordering
    /// of a device reply and a later main-thread callback.
    fn set_window_state(&mut self, open: bool) {
        if self.window_open != open {
            self.window_open = open;
            self.activity
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .window_state = Some(open);
        }
    }

    /// Files a received message in arrival order. Activity goes to its own
    /// storage; a dirty notice stays for the background save and, while the
    /// window is open, also counts as a change the user made in the plugin.
    /// Window replies update the open state here, not when a request takes
    /// them, so a notice right behind the reply sees the new state.
    fn accept(&mut self, id: u64, message: ControlMessage) {
        match &message {
            ControlMessage::GuiShown(_) => self.set_window_state(true),
            ControlMessage::GuiClosedReply => self.set_window_state(false),
            _ => {}
        }
        if id != 0 && !self.pending.contains(&id) {
            return; // A late reply can never satisfy a newer request.
        }
        if let ControlMessage::Ports {
            latency_samples, ..
        }
        | ControlMessage::PluginRestarted { latency_samples } = &message
        {
            let previous = self
                .latency_samples
                .swap(*latency_samples, Ordering::AcqRel);
            if previous != *latency_samples {
                self.activity
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .latency_samples = Some(*latency_samples);
            }
        }
        let note = match message {
            ControlMessage::GuiCommand(command) => PluginActivity {
                commands: vec![command],
                window_open: self.window_open,
                ..PluginActivity::default()
            },
            ControlMessage::ParamRelay(relay) => PluginActivity {
                relay,
                window_open: self.window_open,
                ..PluginActivity::default()
            },
            ControlMessage::GuiClosed => {
                self.set_window_state(false);
                PluginActivity {
                    closed: true,
                    ..PluginActivity::default()
                }
            }
            ControlMessage::Dirty if self.window_open => {
                self.inbox.push_back((0, ControlMessage::Dirty));
                PluginActivity {
                    dirty: true,
                    window_open: true,
                    ..PluginActivity::default()
                }
            }
            other => {
                self.inbox.push_back((id, other));
                return;
            }
        };
        self.activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .merge(note);
    }

    fn remember(&mut self, id: u64, state: AppliedState) {
        if id < self.remembered_request {
            return;
        }
        self.remembered_request = id;
        *self
            .applied
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(state);
    }

    /// Asks the helper to save. The reply is collected later by [`Self::poll_saved`].
    /// The poller does not wait for room: a helper that has stopped reading
    /// leaves the request unsent, and the next poll asks again (ADR 0319).
    pub fn request_save(&mut self) -> Result<(), SessionError> {
        if self.save_requested.is_some() {
            return Err(SessionError::SavePending);
        }
        let id = self.request(ControlMessage::SaveRequest, Instant::now())?;
        self.save_requested = Some(id);
        Ok(())
    }

    /// A save reply already queued, or `None` when the helper has not answered.
    pub fn poll_saved(&mut self) -> Result<Option<Result<Vec<u8>, Refusal>>, SessionError> {
        if let Err(error) = self.drain() {
            if let Some(id) = self.save_requested.take() {
                self.pending.remove(&id);
            }
            return Err(error);
        }
        let Some(id) = self.save_requested else {
            return Ok(None);
        };
        let index = self.inbox.iter().position(|(reply_id, message)| {
            *reply_id == id
                && matches!(
                    message,
                    ControlMessage::State(_) | ControlMessage::StateRejected(_)
                )
        });
        let Some(index) = index else {
            return Ok(None);
        };
        self.save_requested = None;
        self.pending.remove(&id);
        match self.inbox.remove(index) {
            Some((_, ControlMessage::State(bytes))) => {
                self.remember(id, AppliedState::new(bytes.as_slice()));
                Ok(Some(Ok(bytes)))
            }
            Some((_, ControlMessage::StateRejected(reason))) => Ok(Some(Err(reason))),
            _ => Ok(None),
        }
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, SessionError> {
        let deadline = Instant::now() + STATE_REPLY;
        let id = self.request(ControlMessage::SaveRequest, deadline)?;
        match self.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::State(_) | ControlMessage::StateRejected(_)
            )
        })? {
            ControlMessage::State(bytes) => {
                self.remember(id, AppliedState::new(bytes.as_slice()));
                Ok(bytes)
            }
            ControlMessage::StateRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("return state").into()),
        }
    }

    pub fn poll_notice(&mut self) -> Result<Option<PluginNotice>, SessionError> {
        self.drain()?;
        let index = self.inbox.iter().position(|(_, message)| {
            matches!(
                message,
                ControlMessage::Dirty
                    | ControlMessage::PluginRestarted { .. }
                    | ControlMessage::RequestProcess
            )
        });
        let Some(index) = index else {
            return Ok(None);
        };
        Ok(Some(match self.inbox.remove(index) {
            Some((_, ControlMessage::Dirty)) => PluginNotice::Dirty,
            Some((_, ControlMessage::PluginRestarted { latency_samples })) => {
                PluginNotice::PluginRestarted { latency_samples }
            }
            Some((_, ControlMessage::RequestProcess)) => PluginNotice::RequestProcess,
            _ => {
                return Err(ProtocolError::QueueLost.into());
            }
        }))
    }

    fn drain(&mut self) -> Result<(), SessionError> {
        loop {
            match self.control.try_recv_tagged() {
                Ok(Some((id, message))) => self.accept(id, message),
                Ok(None) => return Ok(()),
                // The helper can close after a complete message. That EOF must
                // not discard a reason already sitting in the inbox.
                Err(error) => {
                    self.set_window_state(false);
                    if !self.inbox.is_empty() {
                        return Ok(());
                    }
                    return Err(error.into());
                }
            }
        }
    }

    /// Sends a request that must be on its way by `deadline`, the same one
    /// its reply is waited for: a helper that stops reading cannot hold the
    /// caller in the send (R9, ADR 0319).
    fn request(&mut self, message: ControlMessage, deadline: Instant) -> Result<u64, SessionError> {
        let id = self.allocate_request()?;
        if let Err(error) = self.control.send_tagged_until(id, &message, deadline) {
            self.pending.remove(&id);
            return Err(error.into());
        }
        Ok(id)
    }

    fn allocate_request(&mut self) -> Result<u64, SessionError> {
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(ProtocolError::RequestIdsExhausted)?;
        self.pending.insert(self.next_request);
        Ok(self.next_request)
    }

    fn wait_reply(
        &mut self,
        id: u64,
        deadline: Instant,
        want: impl Fn(&ControlMessage) -> bool,
    ) -> Result<ControlMessage, SessionError> {
        let result = self.wait_matching(id, deadline, want);
        self.pending.remove(&id);
        self.inbox.retain(|(reply_id, _)| *reply_id != id);
        result
    }
    fn wait_message(
        &mut self,
        timeout: Duration,
        want: impl Fn(&ControlMessage) -> bool,
    ) -> Result<ControlMessage, SessionError> {
        self.wait_matching(0, Instant::now() + timeout, want)
    }

    fn wait_matching(
        &mut self,
        id: u64,
        deadline: Instant,
        want: impl Fn(&ControlMessage) -> bool,
    ) -> Result<ControlMessage, SessionError> {
        loop {
            self.drain()?;
            if let Some(index) = self
                .inbox
                .iter()
                .position(|(reply_id, message)| *reply_id == id && want(message))
            {
                return self
                    .inbox
                    .remove(index)
                    .map(|(_, message)| message)
                    .ok_or_else(|| ProtocolError::QueueLost.into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            match self
                .control
                .recv_tagged(remaining.min(Duration::from_millis(50)))
            {
                Ok((id, message)) => self.accept(id, message),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => {
                    self.set_window_state(false);
                    return Err(error.into());
                }
            }
        }
    }
}

/// A submitted block, looked at without waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockProgress {
    Ready { events: usize },
    Pending,
    Abandoned,
}

impl PluginHost {
    pub fn spawn(mut options: SpawnOptions<'_>) -> Result<Self, SessionError> {
        crate::descriptors::raise_descriptor_limit();
        let workgroup = options
            .workgroup
            .take()
            .map(crate::RetainedWorkgroup::from_handoff)
            .transpose()
            .map_err(SessionError::Workgroup)?;
        let region = Arc::new(MappedRegion::create_shared(options.layout)?);
        let shm = region.as_raw_fd();
        let launched = launch(
            options.host_bin,
            options.plugin,
            options.plugin_id,
            options.sample_rate,
            shm,
            workgroup
                .as_ref()
                .map(crate::RetainedWorkgroup::handoff)
                .transpose()
                .map_err(SessionError::Workgroup)?,
            options.fault,
            options.fan_in.as_deref(),
        )?;
        let stderr = Arc::new(Mutex::new(String::new()));
        spawn_stderr_reader(launched.stderr, Arc::clone(&stderr));
        let watch_slots = Arc::new(Mutex::new(WatchSlots {
            notice: region.notice_ptr(),
            dead: region.dead_ptr(),
            fan: std::ptr::null(),
        }));
        let watch_stop = Arc::new(AtomicBool::new(false));
        let mut host = Self {
            workgroup,
            root: Arc::clone(&region),
            previous_region: None,
            region,
            child: launched.child,
            control: PluginControl::new(launched.control),
            stderr,
            closed: false,
            reset_pending: false,
            host_bin: options.host_bin.to_path_buf(),
            plugin: options.plugin.map(Path::to_path_buf),
            plugin_id: options.plugin_id.map(str::to_string),
            sample_rate: options.sample_rate,
            watch: None,
            watch_slots,
            watch_stop,
            fan: options.fan_in.clone(),
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            tail_samples: 0,
        };
        host.start_watch();
        if let Err(error) = host.finish_launch() {
            host.force_stop();
            return Err(host.launch_error(error));
        }
        Ok(host)
    }

    /// Start a new helper with fresh, unlinked storage. Old wake targets must
    /// not retain a chain of previous process mappings across port changes.
    pub fn restart(&mut self) -> Result<(), SessionError> {
        self.stop_watch();
        if self.child.try_wait()?.is_none() {
            self.force_stop();
        }
        self.region = Arc::new(MappedRegion::create_shared(self.region.layout())?);
        self.root = Arc::clone(&self.region);
        self.previous_region = None;
        self.point_watch();
        {
            let mut control = self.control.lock();
            control.set_window_state(false);
            control.inbox.clear();
            control.pending.clear();
            control.remembered_request = 0;
            control.save_requested = None;
            control.dirty_again = false;
        }
        self.closed = false;
        let fan = self.fan.clone();
        let launched = launch(
            &self.host_bin,
            self.plugin.as_deref(),
            self.plugin_id.as_deref(),
            self.sample_rate,
            self.region.as_raw_fd(),
            self.workgroup
                .as_ref()
                .map(crate::RetainedWorkgroup::handoff)
                .transpose()
                .map_err(SessionError::Workgroup)?,
            None,
            fan.as_deref(),
        )?;
        spawn_stderr_reader(launched.stderr, Arc::clone(&self.stderr));
        self.child = launched.child;
        self.control.lock().control = launched.control;
        self.start_watch();
        self.finish_launch()?;
        let state = self.control.applied_state();
        if let Some(state) = state {
            self.control.restore_applied(&state)?;
        }
        Ok(())
    }

    pub fn layout(&self) -> SessionLayout {
        self.region.layout()
    }

    /// Channel counts of the plugin's input ports, in declaration order.
    pub fn input_ports(&self) -> &[u32] {
        &self.input_ports
    }

    /// Channel counts of the plugin's output ports, in declaration order.
    pub fn output_ports(&self) -> &[u32] {
        &self.output_ports
    }

    pub fn stderr_log(&self) -> String {
        self.stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    /// The helper's fatal-signal handler has stored its mark.
    /// Exit without that handler leaves this false.
    pub fn fatal_notice(&self) -> bool {
        self.root.fatal_notice() || self.region.fatal_notice()
    }

    pub fn process(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        self.exchange(CMD_PROCESS, input, output, timeout)
    }

    /// Create an endpoint for audio-owned storage.
    ///
    /// # Safety
    /// Only one endpoint may submit or collect a mapping at a time. Standalone
    /// host processing, synchronous remap, and restart must not overlap endpoint
    /// use. Keep this host alive until endpoint use ends. Preparing a new mapping
    /// is allowed while the previous endpoint processes its old mapping.
    pub unsafe fn audio_handle(&self) -> PluginAudio {
        PluginAudio {
            region: Arc::clone(&self.region),
            root: Arc::clone(&self.root),
            reset_pending: false,
        }
    }

    pub fn request_reset(&mut self) {
        self.reset_pending = true;
    }

    pub fn submit(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        events: &[(Event, u32)],
    ) -> Result<u64, SessionError> {
        // SAFETY: standalone calls hold exclusive host access. Endpoint users
        // must exclude standalone processing under audio_handle's contract.
        let mut audio = unsafe { self.audio_handle() };
        audio.reset_pending = self.reset_pending;
        let result = audio.submit(frames, input, events);
        self.reset_pending = audio.reset_pending;
        result
    }

    pub fn submit_planes(
        &mut self,
        frames: usize,
        input: &[Vec<f32>],
        events: &[(Event, u32)],
    ) -> Result<u64, SessionError> {
        // SAFETY: standalone calls hold exclusive host access. Endpoint users
        // must exclude standalone processing under audio_handle's contract.
        let mut audio = unsafe { self.audio_handle() };
        audio.reset_pending = self.reset_pending;
        let result = audio.submit_planes(frames, input, events);
        self.reset_pending = audio.reset_pending;
        result
    }

    pub fn poll(
        &mut self,
        generation: u64,
        output: &mut [&mut [f32]],
        delivered: &mut [Delivered],
    ) -> Result<BlockProgress, SessionError> {
        // SAFETY: exclusive standalone host access; see audio_handle.
        unsafe { self.audio_handle() }.poll(generation, output, delivered)
    }

    pub fn poll_planes(
        &mut self,
        generation: u64,
        output: &mut [Vec<f32>],
        frames: usize,
        delivered: &mut [Delivered],
    ) -> Result<BlockProgress, SessionError> {
        // SAFETY: exclusive standalone host access; see audio_handle.
        unsafe { self.audio_handle() }.poll_planes(generation, output, frames, delivered)
    }

    pub fn abandon(&mut self, generation: u64) {
        self.region.abandon(generation);
    }

    /// The helper process has exited. Reaps it. Control thread only: this calls
    /// `waitpid`, which the audio callback must not do.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Ask the helper to run `stages` process calls, in order, for each block.
    pub fn set_chain(&mut self, stages: u32) -> Result<(), SessionError> {
        let mut control = self.control.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = control.request(ControlMessage::SetChain(stages), deadline)?;
        match control.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::ChainReady | ControlMessage::ChainRejected
            )
        })? {
            ControlMessage::ChainReady => Ok(()),
            ControlMessage::ChainRejected => Err(SessionError::ChainRefused),
            _ => Err(ProtocolError::UnexpectedReply("answer the chain").into()),
        }
    }

    pub fn process_events(
        &mut self,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        let frames = if let Some(plane) = input.first() {
            plane.len()
        } else if let Some(plane) = output.first() {
            plane.len()
        } else {
            return Err(RegionError::BadShape {
                frames: 0,
                channels: 0,
            }
            .into());
        };
        self.exchange_events(
            frames,
            input,
            output,
            events_in,
            events_out,
            CMD_PROCESS,
            timeout,
        )
    }

    /// `frames` is required when the session has no audio channels.
    pub fn process_block(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        self.exchange_events(
            frames,
            input,
            output,
            events_in,
            events_out,
            CMD_PROCESS,
            timeout,
        )
    }

    /// A control-only endpoint. Polling it never borrows the audio mapping.
    pub fn control_handle(&self) -> PluginControl {
        self.control.clone()
    }

    pub fn poll_notice(&mut self) -> Result<Option<PluginNotice>, SessionError> {
        self.control.lock().poll_notice()
    }

    fn wait_message(
        &mut self,
        timeout: Duration,
        want: impl Fn(&ControlMessage) -> bool,
    ) -> Result<ControlMessage, SessionError> {
        self.control.lock().wait_message(timeout, want)
    }

    /// Prepare replacement storage while the current mapping keeps processing.
    /// The first request on the new endpoint activates it at a block boundary.
    pub fn prepare_mapping(&mut self, layout: SessionLayout) -> Result<(), SessionError> {
        if self.root.is_dead() {
            return Err(SessionError::HelperDown);
        }
        if layout == self.layout() {
            return Ok(());
        }
        if !self.region.is_ready() && self.previous_region.is_some() {
            return Err(SessionError::MappingPending);
        }
        let mut region = MappedRegion::create_shared(layout)?;
        region.set_wake_target(Arc::clone(&self.root));
        let fd = region.duplicate_fd()?;
        let mut control = self.control.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = control.request(ControlMessage::Remap { layout, fd }, deadline)?;
        let reply = control.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::Remapped | ControlMessage::RemapFailed
            )
        })?;
        drop(control);
        match reply {
            ControlMessage::Remapped => {
                self.previous_region = Some(std::mem::replace(&mut self.region, Arc::new(region)));
                Ok(())
            }
            ControlMessage::RemapFailed => Err(SessionError::RemapRefused),
            _ => Err(ProtocolError::UnexpectedReply("answer the remap").into()),
        }
    }

    pub fn mapping_pending(&self) -> bool {
        !self.region.is_ready() && !self.root.is_dead()
    }

    /// Undo a prepared mapping that has never been published to the callback.
    pub fn cancel_mapping(&mut self) -> Result<(), SessionError> {
        if self.region.is_ready() || self.previous_region.is_none() {
            return Ok(());
        }
        if self.region.issued() != 0 {
            return Err(SessionError::MappingPending);
        }
        let mut control = self.control.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = control.request(ControlMessage::CancelRemap, deadline)?;
        control.wait_reply(id, deadline, |message| {
            matches!(message, ControlMessage::RemapCancelled)
        })?;
        drop(control);
        if let Some(previous) = self.previous_region.take() {
            self.region = previous;
        }
        Ok(())
    }

    /// Synchronous control-only mapping change for standalone callers.
    pub fn remap(&mut self, layout: SessionLayout) -> Result<(), SessionError> {
        self.prepare_mapping(layout)?;
        if !self.region.is_ready() {
            self.region.activate_mapping(Duration::from_secs(2))?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if self.child.try_wait()?.is_none() {
            let _ = self.send_shutdown();
            if self.child.try_wait()?.is_none() {
                self.force_stop();
            }
        }
        self.stop_watch();
        Ok(())
    }

    fn send_shutdown(&mut self) -> Result<(), SessionError> {
        let layout = self.region.layout();
        let frames = 1;
        let inputs = vec![vec![0.0f32; frames]; layout.input_channels as usize];
        let mut outputs = vec![vec![0.0f32; frames]; layout.output_channels as usize];
        let input_refs: Vec<&[f32]> = inputs.iter().map(|plane| plane.as_slice()).collect();
        let mut output_refs: Vec<&mut [f32]> = outputs
            .iter_mut()
            .map(|plane| plane.as_mut_slice())
            .collect();
        let mut none = [];
        self.exchange_events(
            frames,
            &input_refs,
            &mut output_refs,
            &[],
            &mut none,
            CMD_SHUTDOWN,
            Duration::from_millis(200),
        )
        .map(|_| ())
    }

    fn exchange(
        &mut self,
        command: u32,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        timeout: Duration,
    ) -> Result<(), SessionError> {
        let command = if command == CMD_PROCESS && self.reset_pending {
            CMD_RESET_PROCESS
        } else {
            command
        };
        match self.region.request(0, input, output, command, timeout) {
            Ok(()) => {
                if command == CMD_RESET_PROCESS {
                    self.reset_pending = false;
                }
                Ok(())
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    #[allow(clippy::too_many_arguments)] // planes, events, command, and the deadline are one block
    fn exchange_events(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        command: u32,
        timeout: Duration,
    ) -> Result<usize, SessionError> {
        let command = if command == CMD_PROCESS && self.reset_pending {
            CMD_RESET_PROCESS
        } else {
            command
        };
        match self.region.request_events(
            frames, input, output, events_in, events_out, command, timeout,
        ) {
            Ok(count) => {
                if command == CMD_RESET_PROCESS {
                    self.reset_pending = false;
                }
                Ok(count)
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    fn fail(&mut self, error: RegionError) -> SessionError {
        match error {
            RegionError::Wait(crate::sync::WaitError::Timeout) => self.fail_timeout(),
            RegionError::Dead => self.reap_dead(),
            RegionError::Busy => SessionError::Missed,
            other => other.into(),
        }
    }

    /// Reads the plugin's declared ports, resizes the mapping, then waits until
    /// the helper is ready to process that layout.
    /// Tail declared at activation, in device frames. Values at least i32::MAX are infinite.
    pub fn tail_samples(&self) -> u32 {
        self.tail_samples
    }

    fn finish_launch(&mut self) -> Result<(), SessionError> {
        if self.plugin.is_some() {
            self.align_declared_ports()?;
        }
        self.wait_until_ready(Duration::from_secs(5))
    }

    fn align_declared_ports(&mut self) -> Result<(), SessionError> {
        let (inputs, outputs) = match self.wait_message(Duration::from_secs(5), |message| {
            matches!(
                message,
                ControlMessage::Ports { .. } | ControlMessage::PortsRejected(_)
            )
        })? {
            ControlMessage::Ports {
                inputs,
                outputs,
                tail_samples,
                ..
            } => {
                self.tail_samples = tail_samples;
                (inputs, outputs)
            }
            ControlMessage::PortsRejected(refusal) => return Err(refusal.into()),
            _ => return Err(ProtocolError::UnexpectedReply("describe audio ports").into()),
        };
        let input_channels = sum_channels(&inputs)?;
        let output_channels = sum_channels(&outputs)?;
        self.input_ports = inputs;
        self.output_ports = outputs;
        let layout = self.region.layout();
        if layout.input_channels == input_channels && layout.output_channels == output_channels {
            return Ok(());
        }
        let mut next = layout;
        next.input_channels = input_channels;
        next.output_channels = output_channels;
        self.remap(next)
    }

    fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), SessionError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Err(self.exited(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout);
            }
            let slice = remaining.min(Duration::from_millis(20));
            match self.region.wait_ready(slice) {
                Ok(()) => return Ok(()),
                Err(RegionError::Wait(crate::sync::WaitError::Timeout)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn launch_error(&self, error: SessionError) -> SessionError {
        let mut stderr = self.stderr_log();
        if stderr.trim().is_empty() {
            std::thread::sleep(Duration::from_millis(30));
            stderr = self.stderr_log();
        }
        let stderr = stderr.trim();
        if stderr.is_empty() {
            error
        } else {
            SessionError::Launch {
                error: Box::new(error),
                stderr: stderr.to_string(),
            }
        }
    }

    fn fail_timeout(&mut self) -> SessionError {
        match self.child.try_wait() {
            Ok(Some(status)) => self.exited(status),
            Ok(None) => SessionError::Timeout,
            Err(error) => SessionError::Io(error),
        }
    }

    fn reap_dead(&mut self) -> SessionError {
        match self.child.try_wait() {
            Ok(Some(status)) => self.exited(status),
            Ok(None) => match self.child.wait() {
                Ok(status) => self.exited(status),
                Err(error) => SessionError::Io(error),
            },
            Err(error) => SessionError::Io(error),
        }
    }

    fn exited(&self, status: ExitStatus) -> SessionError {
        let stderr = self.stderr_log();
        // The helper's own output is kept for the log only at its exit, so a
        // plugin that prints all the time cannot fill the log (ADR 0322).
        let tail = stderr_tail(&stderr);
        log::warn!(
            target: "echode_sandbox::session",
            pid = self.child.pid,
            status:% = status,
            plugin = self.plugin_id.as_deref().unwrap_or("none"),
            stderr = tail;
            "a plugin helper exited"
        );
        SessionError::HostExited { status, stderr }
    }

    fn force_stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn start_watch(&mut self) {
        let pid = self.child.os_pid();
        let slots = Arc::clone(&self.watch_slots);
        let stop = Arc::clone(&self.watch_stop);
        if let Ok(mut guard) = self.watch_slots.lock() {
            guard.fan = self
                .fan
                .as_ref()
                .map(|fan| ptr::from_ref(fan.as_ref()))
                .unwrap_or(ptr::null());
        }
        self.watch_stop.store(false, Ordering::Release);
        self.watch = Some(std::thread::spawn(move || watch_exit(pid, slots, stop)));
    }

    fn stop_watch(&mut self) {
        self.watch_stop.store(true, Ordering::Release);
        if let Some(handle) = self.watch.take() {
            let _ = handle.join();
        }
    }

    fn point_watch(&self) {
        if let Ok(mut slots) = self.watch_slots.lock() {
            slots.notice = self.root.notice_ptr();
            slots.dead = self.root.dead_ptr();
        }
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl HelperProcess {
    fn os_pid(&self) -> i32 {
        self.pid
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut raw = 0;
        // SAFETY: `raw` is writable, and `self.pid` is this session's child, which has not been
        // reaped.
        let rc = unsafe { libc::waitpid(self.pid, &mut raw, libc::WNOHANG) };
        if rc == 0 {
            return Ok(None);
        }
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let exit = ExitStatus::from_raw(raw);
        self.status = Some(exit);
        Ok(Some(exit))
    }

    /// Once reaped, the pid may belong to another process, so it is not signalled.
    fn kill(&mut self) -> std::io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: the child has not been reaped, so its pid still names it.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let mut raw = 0;
        loop {
            // SAFETY: `raw` is writable, and `self.pid` is this session's child, which has not been
            // reaped.
            if unsafe { libc::waitpid(self.pid, &mut raw, 0) } >= 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        let exit = ExitStatus::from_raw(raw);
        self.status = Some(exit);
        Ok(exit)
    }
}

/// Consumes the workgroup send right whether or not the launch succeeds.
#[allow(clippy::too_many_arguments)]
fn launch(
    host_bin: &Path,
    plugin: Option<&Path>,
    plugin_id: Option<&str>,
    sample_rate: u32,
    shm: i32,
    workgroup: Option<WorkgroupHandoff>,
    fault: Option<&str>,
    fan: Option<&FanIn>,
) -> Result<Launched, SessionError> {
    launch_with(
        host_bin,
        plugin,
        plugin_id,
        sample_rate,
        shm,
        workgroup.as_ref(),
        fault,
        fan,
    )
}

/// The parent is multithreaded, so the helper is started with `posix_spawn`
/// and never `fork`. A forked child runs libSystem's atfork handlers, and one
/// that finds an initializer another thread was inside dies with SIGKILL.
#[allow(clippy::too_many_arguments)]
fn launch_with(
    host_bin: &Path,
    plugin: Option<&Path>,
    plugin_id: Option<&str>,
    sample_rate: u32,
    shm: i32,
    workgroup: Option<&WorkgroupHandoff>,
    fault: Option<&str>,
    fan: Option<&FanIn>,
) -> Result<Launched, SessionError> {
    if workgroup.is_some_and(|handoff| handoff.period_ticks == 0) {
        return Err(SessionError::Workgroup(WorkgroupError::InvalidHandoff));
    }
    let (parent_socket, child_socket) = Socket::pair()?;
    let mut args = vec![
        OsString::from("--shm-fd"),
        OsString::from("3"),
        OsString::from("--control-fd"),
        OsString::from("4"),
    ];
    let name = workgroup.map(|handoff| {
        let name = crate::workgroup::service_name();
        args.push(OsString::from("--workgroup-name"));
        args.push(OsString::from(&name));
        args.push(OsString::from("--workgroup-ticks"));
        args.push(OsString::from(handoff.period_ticks.to_string()));
        name
    });
    if fan.is_some() {
        args.push(OsString::from("--fan-fd"));
        args.push(OsString::from("5"));
    }
    if let Some(plugin) = plugin {
        args.push(OsString::from("--plugin"));
        args.push(plugin.as_os_str().to_os_string());
    }
    if let Some(plugin_id) = plugin_id {
        args.push(OsString::from("--plugin-id"));
        args.push(OsString::from(plugin_id));
    }
    if sample_rate > 0 {
        args.push(OsString::from("--sample-rate"));
        args.push(OsString::from(sample_rate.to_string()));
    }
    let fault_env;
    let extra: &[(&str, &str)] = if let Some(fault) = fault {
        fault_env = [("ECHODE_TEST_FAULT", fault)];
        &fault_env
    } else {
        &[]
    };
    let shm_dup = inheritable(shm)?;
    let control_dup = inheritable(child_socket.as_raw_fd())?;
    let fan_dup = fan.map(|fan| inheritable(fan.raw_fd())).transpose()?;
    let mut inherit = vec![(shm_dup.as_raw_fd(), 3), (control_dup.as_raw_fd(), 4)];
    if let Some(fan_dup) = fan_dup.as_ref() {
        inherit.push((fan_dup.as_raw_fd(), 5));
    }
    let spawned = spawn_helper(host_bin, &args, extra, &inherit).map_err(SessionError::Spawn)?;
    drop((shm_dup, control_dup, fan_dup, child_socket));
    let mut child = HelperProcess {
        pid: spawned.pid,
        status: None,
    };
    if let (Some(handoff), Some(name)) = (workgroup, name)
        && let Err(error) = crate::workgroup::send_workgroup_port(&name, handoff.port)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SessionError::Workgroup(error));
    }
    Ok(Launched {
        child,
        stderr: Box::new(spawned.stderr),
        control: parent_socket,
    })
}

/// A copy above fds 3, 4, and 5. Those are the child's shared memory, control
/// socket, and fan-in word. A source in that range is replaced by an earlier
/// `dup2`, and the following `close` then drops the descriptor the child needs.
/// `dup2` clears CLOEXEC on the target in the child; the copy keeps it here.
fn inheritable(fd: i32) -> std::io::Result<OwnedFd> {
    // SAFETY: `fcntl` duplicates `fd`, an open descriptor of the caller; no memory is passed.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 6) };
    if dup < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: `fcntl` returned a new descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(dup) })
    }
}

fn sum_channels(counts: &[u32]) -> Result<u32, SessionError> {
    let mut total = 0u32;
    for channels in counts {
        total = total
            .checked_add(*channels)
            .ok_or(ProtocolError::ChannelCountOverflow)?;
    }
    Ok(total)
}

fn spawn_stderr_reader(mut pipe: Box<dyn Read + Send>, slot: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
                    if let Ok(mut text) = slot.lock() {
                        push_stderr(&mut text, &chunk);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

/// The last bytes of a helper's stderr that the log keeps. Bounds this
/// diagnostic copy only; the full capture stays in the error.
const STDERR_LOGGED: usize = 2048;

fn stderr_tail(stderr: &str) -> &str {
    let mut start = stderr.len().saturating_sub(STDERR_LOGGED);
    while !stderr.is_char_boundary(start) {
        start += 1;
    }
    stderr.get(start..).unwrap_or_default().trim()
}

fn push_stderr(slot: &mut String, chunk: &str) {
    slot.push_str(chunk);
    if slot.len() > STDERR_CAP {
        let mut drop_n = slot.len() - STDERR_CAP;
        while drop_n < slot.len() && !slot.is_char_boundary(drop_n) {
            drop_n += 1;
        }
        slot.drain(..drop_n);
    }
}

fn watch_exit(pid: i32, slots: Arc<Mutex<WatchSlots>>, stop: Arc<AtomicBool>) {
    // SAFETY: `kqueue` takes no arguments and returns a new descriptor or -1.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return;
    }
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut event = libc::kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut registered = false;
    while !stop.load(Ordering::Acquire) {
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 200_000_000,
        };
        let changes = if registered { 0 } else { 1 };
        // SAFETY: `change` and `event` are valid for one `kevent` each, and `timeout` is a valid
        // `timespec`.
        let ready = unsafe { libc::kevent(queue, &change, changes, &mut event, 1, &timeout) };
        registered = true;
        if ready > 0 && event.filter == libc::EVFILT_PROC && event.fflags & libc::NOTE_EXIT != 0 {
            if let Ok(guard) = slots.lock() {
                if !guard.dead.is_null() {
                    // SAFETY: `dead` addresses the mapping's flag while this guard is held.
                    unsafe { (*guard.dead).store(1, Ordering::Release) };
                }
                if !guard.notice.is_null() {
                    // SAFETY: `notice` addresses the mapping's wake word. The store
                    // happens before the wake, under the same guard that drops the mapping.
                    unsafe {
                        (*guard.notice).fetch_add(1, Ordering::Release);
                        wake_one(&*guard.notice, SyncFlags::PROCESS);
                    }
                }
                if !guard.fan.is_null() {
                    // SAFETY: `fan` stays mapped while this watcher is joined, which
                    // happens before the host drops it.
                    unsafe { (*guard.fan).bump() };
                }
            }
            break;
        }
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
    }
    // SAFETY: `queue` is the kqueue this function opened, closed once.
    unsafe { libc::close(queue) };
}

/// Realtime endpoint owned by one audio graph. Control code retains the process.
/// Retire this endpoint on the control thread with its graph.
pub struct PluginAudio {
    region: Arc<MappedRegion>,
    root: Arc<MappedRegion>,
    reset_pending: bool,
}

impl PluginAudio {
    /// Callback publication also activates a bank while transport is stopped.
    pub fn activate_mapping(&mut self) -> Result<(), SessionError> {
        if !self.region.is_ready() && self.region.issued() == 0 {
            self.region
                .submit_activation()
                .map_err(|error| self.report_inline(error))?;
        }
        Ok(())
    }

    pub fn carry(&mut self, old: &Self) {
        self.reset_pending = old.reset_pending;
    }
    /// The next submitted block resets processing before receiving its new
    /// events. Parameters and the helper process are retained. No socket IO.
    pub fn request_reset(&mut self) {
        self.reset_pending = true;
    }

    /// Publish one block and return. Does not wait for the helper.
    pub fn submit(
        &mut self,
        frames: usize,
        input: &[&[f32]],
        events: &[(Event, u32)],
    ) -> Result<u64, SessionError> {
        self.check_alive()?;
        match self
            .region
            .submit_block_with_reset(frames, input, events, self.reset_pending)
        {
            Ok(generation) => {
                self.reset_pending = false;
                Ok(generation)
            }
            Err(error) => Err(self.report_inline(error)),
        }
    }

    /// Like [`Self::submit`], taking each channel as its own buffer.
    ///
    /// The callback uses this so it does not allocate a slice of slices.
    pub fn submit_planes(
        &mut self,
        frames: usize,
        input: &[Vec<f32>],
        events: &[(Event, u32)],
    ) -> Result<u64, SessionError> {
        self.check_alive()?;
        match self
            .region
            .submit_planes_with_reset(frames, input, events, self.reset_pending)
        {
            Ok(generation) => {
                self.reset_pending = false;
                Ok(generation)
            }
            Err(error) => Err(self.report_inline(error)),
        }
    }

    /// Read a submitted generation if it has finished. Does not wait.
    pub fn poll(
        &mut self,
        generation: u64,
        output: &mut [&mut [f32]],
        delivered: &mut [Delivered],
    ) -> Result<BlockProgress, SessionError> {
        self.check_alive()?;
        match self.region.collect_ready(generation, output, delivered) {
            Ok(Collected::Pending) => Ok(BlockProgress::Pending),
            Ok(Collected::Abandoned) => Ok(BlockProgress::Abandoned),
            Ok(Collected::Ready { events }) => Ok(BlockProgress::Ready { events }),
            Err(error) => Err(self.report_inline(error)),
        }
    }

    /// Like [`Self::poll`], writing each channel into its own buffer.
    pub fn poll_planes(
        &mut self,
        generation: u64,
        output: &mut [Vec<f32>],
        frames: usize,
        delivered: &mut [Delivered],
    ) -> Result<BlockProgress, SessionError> {
        self.check_alive()?;
        match self
            .region
            .collect_planes(generation, output, frames, delivered)
        {
            Ok(Collected::Pending) => Ok(BlockProgress::Pending),
            Ok(Collected::Abandoned) => Ok(BlockProgress::Abandoned),
            Ok(Collected::Ready { events }) => Ok(BlockProgress::Ready { events }),
            Err(error) => Err(self.report_inline(error)),
        }
    }

    /// The generation will not be read if the helper finishes it later.
    pub fn abandon(&mut self, generation: u64) {
        self.region.abandon(generation);
    }

    fn check_alive(&self) -> Result<(), SessionError> {
        if self.root.is_dead() {
            return Err(self.report_inline(RegionError::Dead));
        }
        Ok(())
    }

    /// Dead, busy, and timeout, without `waitpid` or the stderr lock.
    fn report_inline(&self, error: RegionError) -> SessionError {
        match error {
            RegionError::Wait(crate::sync::WaitError::Timeout) => SessionError::Timeout,
            RegionError::Dead => {
                if self.region.process_refused() || self.root.process_refused() {
                    SessionError::ProcessRefused
                } else {
                    SessionError::HelperDown
                }
            }
            RegionError::Busy => SessionError::Missed,
            other => other.into(),
        }
    }
}

#[path = "session_operations.rs"]
mod operations;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_correlation_tests.rs"]
mod correlation_tests;

#[cfg(test)]
#[path = "session_operations_tests.rs"]
mod operations_tests;

#[cfg(test)]
#[path = "session_refusal_tests.rs"]
mod refusal_tests;

#[cfg(test)]
#[path = "session_decode_tests.rs"]
mod decode_tests;

#[cfg(test)]
#[path = "session_payload_tests.rs"]
mod payload_tests;
