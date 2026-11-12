use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use echode_engine::{
    ClipLoader, ForbidAlloc, PlaybackAudio, PlaybackCommand, PlaybackControl, PlaybackOptions,
    PlaybackStatus, PluginActivity, PluginControl, PluginRoute, PluginState, PluginWorkgroup,
    StateReader, playback, tick_to_sample,
};
use echode_model::{Project, Tick};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};

use crate::io_cycle::{IoCycle, host_now};
use crate::panic_log::CallbackPanic;
use crate::{
    AudioError, CoreAudioCall, Internal, OutputDevice, Rejected, Stopped, TransportCancellation,
    hardware,
};

struct Render {
    player: PlaybackAudio,
    declick: echode_engine::OutputDeclick,
    output_revision: u64,
    enabled: bool,
    left: Vec<f32>,
    right: Vec<f32>,
}

struct Callback {
    render: UnsafeCell<Render>,
    failed: AtomicBool,
    /// Catches a panic in the block's body; HAL never sees it unwind.
    panic: CallbackPanic,
    callbacks: AtomicU64,
    trace: UnsafeCell<Option<crate::trace::Writer>>,
    channels: u32,
    /// Prepared output rate and safety offset. The callback does not ask the device.
    cycle: IoCycle,
    silent: bool,
    hardware_epoch: Arc<AtomicU64>,
    observed_epoch: AtomicU64,
    cancellation: TransportCancellation,
    transport_revision: AtomicU64,
}

// SAFETY: HAL serializes calls to this IOProc. Only that callback touches
// render and the optional trace writer; control frees them after stopping and
// destroying the IOProc. Trace storage itself travels through ownership rings.
unsafe impl Sync for Callback {}

pub(crate) struct TransportSetup {
    pub(crate) loop_ticks: Option<(i64, i64)>,
    pub(crate) metronome: bool,
    pub(crate) clips: ClipLoader,
    pub(crate) package: Option<std::path::PathBuf>,
    pub(crate) cancellation: TransportCancellation,
    pub(crate) revision: u64,
    pub(crate) fader_preview: Option<Vec<(u128, f32, f32)>>,
    /// False opens helpers and the device without starting the transport.
    pub(crate) start: bool,
}

impl TransportSetup {
    fn idle() -> Self {
        Self {
            loop_ticks: None,
            metronome: false,
            clips: Arc::new(|_project, _rate| Ok(Vec::new())),
            package: None,
            cancellation: TransportCancellation::default(),
            revision: 0,
            fader_preview: None,
            start: true,
        }
    }
}

pub struct OutputStream {
    workgroup_listener: Option<hardware::Notifications>,
    pub device: OutputDevice,
    control: PlaybackControl,
    callback: Option<Box<Callback>>,
    proc: AudioDeviceIOProcID,
    /// False for a headless stream, whose callback HAL never calls.
    registered: bool,
    package: Option<std::path::PathBuf>,
    pending_locate: Option<(u64, i64, i64)>,
    /// Play waits until this plan, carrying the edits made while this output
    /// opened, reaches the callback's queue (ADR 0333).
    start_after: Option<u64>,
    trace_files: Option<crate::trace::Files>,
}

/// A checked output device, before any helper starts for it.
pub(crate) struct Claim {
    pub(crate) device: OutputDevice,
    /// Registered before the workgroup lookup, so a changed group cannot
    /// escape invalidation while helpers start.
    pub(crate) listener: Option<hardware::Notifications>,
    pub(crate) workgroup: Option<PluginWorkgroup>,
}

impl Claim {
    /// The device's checks and workgroup. HAL calls only; helpers start later.
    pub(crate) fn hal(device: OutputDevice) -> Result<Self, AudioError> {
        hardware::validate_streams(&device)?;
        let listener = hardware::Notifications::workgroup(device.id)?;
        let workgroup = crate::workgroup::selected(&device)?;
        Ok(Self {
            device,
            listener: Some(listener),
            workgroup: Some(workgroup),
        })
    }

    pub(crate) fn workgroup_changed(&self) -> bool {
        self.listener
            .as_ref()
            .is_some_and(hardware::Notifications::changed)
    }
}

/// Helpers started and media prepared for a claimed device. The slow part of
/// opening an output; it needs no HAL call and may run on any thread.
pub(crate) struct Prepared {
    control: PlaybackControl,
    player: PlaybackAudio,
}

impl Prepared {
    pub(crate) fn new(
        project: &Project,
        target: &Target,
        tick: i64,
        transport: &TransportSetup,
    ) -> Result<Self, AudioError> {
        let (control, player) = prepare_playback_mode(
            project,
            target.sample_rate,
            target.frames,
            tick,
            transport,
            target.workgroup.clone(),
        )?;
        Ok(Self { control, player })
    }

    /// As [`OutputStream::set_test_plugin_routes`], before installation.
    #[cfg(test)]
    pub(crate) fn set_test_plugin_routes(
        &mut self,
        host: &std::path::Path,
        routes: &[PluginRoute],
    ) {
        self.control.set_routes(host, routes);
    }
}

/// What preparation needs from a claim. Unlike the claim, it can be sent.
#[derive(Clone)]
pub(crate) struct Target {
    pub(crate) sample_rate: u32,
    pub(crate) frames: usize,
    pub(crate) workgroup: Option<PluginWorkgroup>,
}

impl Claim {
    pub(crate) fn target(&self) -> Target {
        Target {
            sample_rate: self.device.sample_rate,
            frames: self.device.buffer_frames as usize,
            workgroup: self.workgroup.clone(),
        }
    }
}

/// The transport a prepared stream starts its callback with.
pub(crate) struct Start {
    pub(crate) cancellation: TransportCancellation,
    pub(crate) revision: u64,
    pub(crate) package: Option<std::path::PathBuf>,
    pub(crate) hardware_epoch: Arc<AtomicU64>,
    pub(crate) observed_epoch: u64,
    pub(crate) silent: bool,
}

impl OutputStream {
    pub(crate) fn preview_faders(&mut self, values: &[(u128, f32, f32)]) -> Result<(), AudioError> {
        Ok(self.control.preview_faders(values)?)
    }
    pub(crate) fn clear_fader_preview(&mut self, committed: bool) -> Result<(), AudioError> {
        Ok(self.control.clear_fader_preview(committed)?)
    }
    pub(crate) fn meters(&self) -> Vec<Arc<echode_engine::TrackMeter>> {
        self.control.meters()
    }
    pub fn open(device: OutputDevice, project: &Project, tick: i64) -> Result<Self, AudioError> {
        Self::open_mode(device, project, tick, false)
    }

    /// Opens on this thread, helpers included.
    fn open_mode(
        device: OutputDevice,
        project: &Project,
        tick: i64,
        silent: bool,
    ) -> Result<Self, AudioError> {
        let transport = TransportSetup::idle();
        let claim = Claim::hal(device)?;
        let prepared = Prepared::new(project, &claim.target(), tick, &transport)?;
        Self::start(
            claim,
            prepared,
            Start {
                cancellation: transport.cancellation,
                revision: transport.revision,
                package: transport.package,
                hardware_epoch: Arc::new(AtomicU64::new(0)),
                observed_epoch: 0,
                silent,
            },
        )
    }

    /// Registers and starts HAL's callback around prepared playback.
    pub(crate) fn start(
        claim: Claim,
        prepared: Prepared,
        start: Start,
    ) -> Result<Self, AudioError> {
        let mut value = Self::assemble(claim, prepared, start);
        value.register()?;
        value.prepare_in_background();
        Ok(value)
    }

    /// Starts HAL's callback. Commands sent before this reach its first block.
    pub(crate) fn register(&mut self) -> Result<(), AudioError> {
        if self.registered {
            return Ok(());
        }
        let mut proc = None;
        let Some(callback) = self.callback.as_mut() else {
            return Err(Internal::CallbackMissing.into());
        };
        let context = (&mut **callback as *mut Callback).cast();
        // SAFETY: the boxed context has a stable address and out_proc is SDK typed.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                self.device.id,
                Some(render),
                context,
                NonNull::from(&mut proc),
            )
        };
        if status != 0 {
            return Err(AudioError::CoreAudio {
                call: CoreAudioCall::CreateOutputProc,
                status,
            });
        }
        self.proc = proc;
        self.registered = true;
        // SAFETY: `self.proc` is the IOProc just created on this device.
        let status = unsafe { AudioDeviceStart(self.device.id, self.proc) };
        if status != 0 {
            return Err(AudioError::CoreAudio {
                call: CoreAudioCall::StartOutput,
                status,
            });
        }
        Ok(())
    }

    /// Replacement helpers prepare on another thread from now on, while this
    /// output keeps running: installation enables it before it sends the
    /// edits made while the output opened (ADR 0142, ADR 0333).
    pub(crate) fn prepare_in_background(&mut self) {
        self.control.prepare_in_background();
    }
    /// Tests can exercise unresolved-device graph publication even when a
    /// sanitizer's build directory has no installed helper executable.
    #[cfg(test)]
    pub(crate) fn set_test_plugin_routes(
        &mut self,
        host: &std::path::Path,
        routes: &[PluginRoute],
    ) {
        self.control.set_routes(host, routes);
    }

    pub(crate) fn preparing_graph(&self) -> bool {
        self.control.preparing_graph()
    }

    /// The newest replacement's media has not reached the callback's queue.
    pub(crate) fn loading_media(&self) -> bool {
        self.control.loading_media()
    }

    /// Play waits for the plan with every edit requested so far (ADR 0333).
    /// Later plans adopt while playing, as for any open output (ADR 0142).
    pub(crate) fn hold_start_for_requested_plan(&mut self) {
        self.start_after = Some(self.control.requested_plan());
    }

    /// Play waits: for media still loading (ADR 0330), or for the plan
    /// [`OutputStream::hold_start_for_requested_plan`] named.
    pub(crate) fn holding_start(&self) -> bool {
        self.loading_media()
            || self
                .start_after
                .is_some_and(|plan| !self.control.published(plan))
    }
    /// The stream with its callback built but not registered with HAL.
    pub(crate) fn assemble(claim: Claim, prepared: Prepared, start: Start) -> Self {
        let Claim {
            device, listener, ..
        } = claim;
        let Prepared {
            mut control,
            player,
        } = prepared;
        // The initial plan already has its media. Every later replacement
        // decodes new media on another thread, playing or not, and keeps the
        // current plan sounding meanwhile (ADR 0147, ADR 0330).
        control.load_media_in_background(true);
        let frames = device.buffer_frames as usize;
        let (trace, trace_files) = crate::trace::configured(device.sample_rate, frames)
            .map_or((None, None), |(writer, files)| (Some(writer), Some(files)));
        let callback = Box::new(Callback {
            render: UnsafeCell::new(Render {
                player,
                declick: echode_engine::OutputDeclick::new(device.sample_rate),
                output_revision: 0,
                enabled: true,
                left: vec![0.0; frames],
                right: vec![0.0; frames],
            }),
            failed: AtomicBool::new(false),
            panic: CallbackPanic::new("output callback"),
            callbacks: AtomicU64::new(0),
            trace: UnsafeCell::new(trace),
            channels: device.channels,
            cycle: IoCycle::new(device.sample_rate, device.safety_frames),
            silent: start.silent,
            hardware_epoch: start.hardware_epoch,
            observed_epoch: AtomicU64::new(start.observed_epoch),
            cancellation: start.cancellation,
            transport_revision: AtomicU64::new(start.revision),
        });
        Self {
            workgroup_listener: listener,
            device,
            control,
            callback: Some(callback),
            proc: None,
            registered: false,
            package: start.package,
            pending_locate: None,
            start_after: None,
            trace_files,
        }
    }

    /// Runs the callback once with owned buffers, as HAL would.
    #[cfg(test)]
    pub(crate) fn pump(&self, frames: usize) -> Vec<f32> {
        let channels = self.device.channels.max(1) as usize;
        let mut data = vec![0.0f32; frames * channels];
        let buffer = AudioBuffer {
            mNumberChannels: channels as u32,
            mDataByteSize: (data.len() * size_of::<f32>()) as u32,
            mData: data.as_mut_ptr().cast(),
        };
        if let Some(callback) = &self.callback {
            // SAFETY: the test calls this in place of HAL, never alongside it, and `buffer` points
            // at `data`, writable for its byte size.
            unsafe { fill(callback, &mut [buffer], Instant::now()) };
        }
        data
    }

    /// `live` names devices whose helpers hold a change the document has not
    /// recorded yet. Their routes carry the state those helpers already have,
    /// so the replacement keeps them instead of restoring older bytes (ADR 0229).
    /// Recorded states are named, not read: a helper that already holds its
    /// state needs nothing from the store (ADR 0329).
    pub fn replace(&mut self, project: &Project, live: &[u128]) -> Result<(), AudioError> {
        if let Some(host) = echode_plugin_host::host_binary() {
            let held = |device: u128| {
                live.contains(&device)
                    .then(|| self.control.device_applied_state(device).ok())
                    .flatten()
                    .map(|applied| applied.as_ref().map(PluginState::held))
            };
            let routes = plugin_routes(project, self.package.as_deref(), held);
            self.control.set_routes(&host, &routes);
        }
        Ok(self.control.replace(project)?)
    }

    pub(crate) fn resume(&mut self, revision: u64) -> Result<(), AudioError> {
        self.control.command(PlaybackCommand::Play)?;
        if let Some(callback) = &self.callback {
            callback
                .transport_revision
                .store(revision, Ordering::Release);
        }
        Ok(())
    }

    pub(crate) fn pause(&mut self) -> Result<(), AudioError> {
        Ok(self.control.command(PlaybackCommand::Pause)?)
    }

    pub fn set_loop(&mut self, project: &Project, start: i64, end: i64) -> Result<(), AudioError> {
        if end <= start {
            return Err(Rejected::Loop.into());
        }
        let start = loop_sample(project, self.device.sample_rate, start)?;
        let end = loop_sample(project, self.device.sample_rate, end)?;
        if end <= start {
            return Err(Rejected::Loop.into());
        }
        Ok(self
            .control
            .command(PlaybackCommand::SetLoop { start, end })?)
    }

    pub fn clear_loop(&mut self) -> Result<(), AudioError> {
        Ok(self.control.command(PlaybackCommand::ClearLoop)?)
    }

    pub fn set_metronome(&mut self, enabled: bool) -> Result<(), AudioError> {
        Ok(self
            .control
            .command(PlaybackCommand::SetMetronome(enabled))?)
    }

    /// The callback converts `tick` under the plan it plays when it takes the
    /// command, so a locate during a tempo edit's preparation lands on the
    /// beat (ADR 0330). `project` checks the position and gives the sample
    /// reported until then.
    pub fn locate(&mut self, project: &Project, tick: i64) -> Result<(), AudioError> {
        let sample = project_sample(project, self.device.sample_rate, tick)?;
        self.control.command(PlaybackCommand::LocateTick(tick))?;
        self.pending_locate = Some((self.control.command_revision(), tick, sample));
        Ok(())
    }

    /// Stops at `tick` without resetting plugins, for a stop that returns to
    /// the remembered start. Tails continue from where playback stopped.
    pub(crate) fn rewind(&mut self, project: &Project, tick: i64) -> Result<(), AudioError> {
        let sample = project_sample(project, self.device.sample_rate, tick)?;
        self.control.command(PlaybackCommand::RewindTick(tick))?;
        self.pending_locate = Some((self.control.command_revision(), tick, sample));
        Ok(())
    }

    pub fn take_plugin_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        self.control.take_plugin_saves()
    }

    pub fn take_plugin_activity(&self) -> Vec<(u128, PluginActivity)> {
        self.control.take_plugin_activity()
    }

    /// The control endpoint of the helper hosting `device`.
    pub fn device_control(&self, device: u128) -> Result<PluginControl, AudioError> {
        Ok(self.control.device_control_handle(device)?)
    }

    /// Brings the helpers of `devices` to the project's recorded plugin states,
    /// for an Undo, Redo or API edit that changed only those hashes.
    pub fn sync_plugin_states(
        &mut self,
        project: &Project,
        devices: &[u128],
        live: &[u128],
    ) -> Result<(), AudioError> {
        let reader = self.package.clone().map(state_reader);
        let recorded = |device: u128| -> Result<Option<PluginState>, AudioError> {
            let state = project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .find(|item| item.id.raw() == device)
                .and_then(|item| item.state.clone());
            match (state, &reader) {
                (None, _) => Ok(None),
                (Some(hash), Some(reader)) => {
                    Ok(Some(PluginState::stored(hash, Arc::clone(reader))))
                }
                (Some(_), None) => Err(AudioError::NoPackage),
            }
        };
        let replace = self.control.restore_recorded_states(devices, recorded);
        if self.control.preparing_graph() || replace {
            self.replace(project, live)
        } else {
            Ok(())
        }
    }

    pub fn service(&mut self) -> Result<(), AudioError> {
        if self.workgroup_changed() {
            return Err(Stopped::WorkgroupChanged.into());
        }
        self.control.service();
        if self
            .start_after
            .is_some_and(|plan| self.control.published(plan))
        {
            self.start_after = None;
        }
        if let Some(files) = &mut self.trace_files {
            files.service(self.control.status());
        }
        if let Some(error) = self.control.take_preparation_error() {
            return Err(error.into());
        }
        if let Some(reason) = self
            .callback
            .as_ref()
            .and_then(|callback| callback.panic.reason())
        {
            return Err(Stopped::CallbackPanicked(reason.into()).into());
        }
        if self.control.status().failed
            || self
                .callback
                .as_ref()
                .is_some_and(|callback| callback.failed.load(Ordering::Acquire))
        {
            return Err(Stopped::DeviceChanged.into());
        }
        Ok(())
    }

    pub fn status(&self) -> PlaybackStatus {
        let mut status = self.control.status();
        if let Some((revision, tick, sample)) = self.pending_locate
            // Sequence arithmetic also covers wraparound; fewer than half of
            // u64's revisions can be in the transport ring simultaneously.
            && status.applied_command.wrapping_sub(revision) > u64::MAX / 2
            && !status.failed
        {
            status.tick = tick;
            status.sample = sample;
        }
        status
    }
    pub(crate) fn workgroup_changed(&self) -> bool {
        self.workgroup_listener
            .as_ref()
            .is_some_and(hardware::Notifications::changed)
    }

    pub fn callbacks(&self) -> u64 {
        self.callback
            .as_ref()
            .map_or(0, |callback| callback.callbacks.load(Ordering::Acquire))
    }

    pub(crate) fn acknowledge_configuration(&self, epoch: u64) {
        if let Some(callback) = &self.callback {
            callback.observed_epoch.store(epoch, Ordering::Release);
        }
    }

    /// Stops callbacks before reading the final playhead. Drop stays on the
    /// control thread, including every plan still in an ownership ring.
    pub fn close(&mut self) -> PlaybackStatus {
        let mut failed = false;
        // The log keeps a panic even when no service call asked for it.
        if let Some(callback) = &self.callback {
            let _ = callback.panic.reason();
        }
        if let Some(callback) = self.callback.take()
            && self.registered
        {
            // SAFETY: `self.proc` is the IOProc `register` created on this device.
            let stopped = unsafe { AudioDeviceStop(self.device.id, self.proc) };
            // SAFETY: `self.proc` is the IOProc `register` created on this device, stopped above.
            let destroyed = unsafe { AudioDeviceDestroyIOProcID(self.device.id, self.proc) };
            if destroyed != 0
                && destroyed != kAudioHardwareBadObjectError
                && destroyed != kAudioHardwareBadDeviceError
            {
                // Preserve the context if HAL could still call it. This is an
                // exceptional driver failure; freeing it would be unsafe.
                Box::leak(callback);
                failed = true;
            }
            let _ = stopped;
        }
        self.control.service();
        let mut status = self.status();
        status.failed |= failed;
        status
    }
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        self.close();
    }
}

fn plugin_routes(
    project: &Project,
    package: Option<&std::path::Path>,
    live: impl Fn(u128) -> Option<Option<PluginState>>,
) -> Vec<PluginRoute> {
    match package {
        Some(package) => routes_with_states(
            project,
            &state_reader(package.to_path_buf()),
            &installed_plugin_routes(project),
            live,
        ),
        None => installed_plugin_routes(project),
    }
}

/// Reads a recorded state from `package` when a helper must load it. The
/// store checks that the bytes have the name they were asked for.
fn state_reader(package: std::path::PathBuf) -> StateReader {
    Arc::new(move |hash| {
        echode_history::read_object(&package, hash).map_err(|error| Arc::new(error) as _)
    })
}

/// Online CLAP ids in project order. A repeated id is listed once.
pub fn project_plugin_ids(project: &Project) -> Vec<String> {
    let mut ids = Vec::new();
    for track in project.tracks() {
        for device in &track.devices {
            if device.offline {
                continue;
            }
            let Some(plugin) = device.plugin.as_ref() else {
                continue;
            };
            if ids.iter().any(|id| id == plugin) {
                continue;
            }
            ids.push(plugin.clone());
        }
    }
    ids
}

/// Installed plugins, in [`project_plugin_ids`] order.
///
/// An id missing from the scan cache is omitted. Playback leaves that device
/// silent. Export must refuse the file instead. Offline devices are not resolved.
pub fn installed_plugin_routes(project: &Project) -> Vec<PluginRoute> {
    echode_plugin_host::resolve_installed(&project_plugin_ids(project))
        .into_iter()
        .map(|plugin| PluginRoute {
            clap_id: plugin.clap_id,
            path: plugin.path,
            instrument: plugin.instrument,
            fault: None,
            state: None,
        })
        .collect()
}

/// One route per online CLAP device, in project order, with its saved state.
///
/// States are read from `package` when a helper loads them, so a hash whose
/// object is missing or does not match fails that helper's start
/// ([`echode_engine::PlaybackError::State`]). Devices that share a plugin id
/// keep separate states so a later device does not reuse the first one's.
pub fn routes_with_saved_state(project: &Project, package: &std::path::Path) -> Vec<PluginRoute> {
    routes_with_states(
        project,
        &state_reader(package.to_path_buf()),
        &installed_plugin_routes(project),
        |_| None,
    )
}

/// [`routes_with_saved_state`] over `installed`, except that a device `live`
/// answers for takes the state its running helper holds (or none) instead of
/// its recorded object.
fn routes_with_states(
    project: &Project,
    reader: &StateReader,
    installed: &[PluginRoute],
    live: impl Fn(u128) -> Option<Option<PluginState>>,
) -> Vec<PluginRoute> {
    let mut routes = Vec::new();
    for track in project.tracks() {
        for device in &track.devices {
            if device.offline {
                continue;
            }
            let Some(id) = device.plugin.as_deref() else {
                continue;
            };
            let Some(found) = installed.iter().find(|route| route.clap_id == id) else {
                continue;
            };
            let mut route = found.clone();
            if let Some(held) = live(device.id.raw()) {
                route.state = held;
            } else if let Some(hash) = &device.state {
                route.state = Some(PluginState::stored(hash.clone(), Arc::clone(reader)));
            }
            routes.push(route);
        }
    }
    routes
}

/// The exact preparation used before HAL registers its callback. Keeping it
/// separate lets tests hold the first callback without an audio device.
#[cfg(test)]
fn prepare_playback(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    tick: i64,
    transport: &TransportSetup,
) -> Result<(PlaybackControl, PlaybackAudio), AudioError> {
    prepare_playback_mode(project, sample_rate, frames, tick, transport, None)
}

fn prepare_playback_mode(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    tick: i64,
    transport: &TransportSetup,
    workgroup: Option<PluginWorkgroup>,
) -> Result<(PlaybackControl, PlaybackAudio), AudioError> {
    let sample = project_sample(project, sample_rate, tick)?;
    let (mut control, player) = open_playback(
        project,
        sample_rate,
        frames,
        transport.package.as_deref(),
        sample,
        workgroup,
    )?;
    control.set_clip_loader(Arc::clone(&transport.clips));
    control.replace(project)?;
    control.command(PlaybackCommand::Locate(sample))?;
    if let Some((start, end)) = transport.loop_ticks {
        let start = loop_sample(project, sample_rate, start)?;
        let end = loop_sample(project, sample_rate, end)?;
        if end <= start {
            return Err(Rejected::Loop.into());
        }
        control.command(PlaybackCommand::SetLoop { start, end })?;
    }
    control.command(PlaybackCommand::SetMetronome(transport.metronome))?;
    if let Some(values) = &transport.fader_preview {
        control.preview_faders(values)?;
    }
    if transport.start {
        control.command(PlaybackCommand::Play)?;
    }
    Ok((control, player))
}

fn open_playback(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    package: Option<&std::path::Path>,
    sample: i64,
    workgroup: Option<PluginWorkgroup>,
) -> Result<(PlaybackControl, PlaybackAudio), AudioError> {
    let host = echode_plugin_host::host_binary();
    let routes = match host {
        Some(_) => plugin_routes(project, package, |_| None),
        None => Vec::new(),
    };
    // A device output mutes a runaway master instead of sending it (ADR 0282).
    let mut options = PlaybackOptions::default().at(sample).guard_output();
    if let Some(host) = &host {
        options = options.plugins(host, &routes);
    }
    if let Some(workgroup) = workgroup {
        options = options.workgroup(workgroup);
    }
    Ok(playback(project, sample_rate, frames, options)?)
}

fn loop_sample(project: &Project, sample_rate: u32, tick: i64) -> Result<i64, AudioError> {
    if tick < 0 {
        return Err(Rejected::Loop.into());
    }
    project_sample(project, sample_rate, tick)
}

/// The device sample of a project tick, or the engine's overflow error.
fn project_sample(project: &Project, sample_rate: u32, tick: i64) -> Result<i64, AudioError> {
    tick_to_sample(Tick::new(tick), project.tempo(), sample_rate)
        .ok_or(AudioError::Playback(echode_engine::PlaybackError::Time))
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
    // Read the host clock after `Instant`, so mapping the output time onto
    // the instant can only end the block earlier.
    let at = Instant::now();
    let host = host_now();
    // SAFETY: the context is the boxed `Callback` given to
    // `AudioDeviceCreateIOProcID`, which `close` frees only after destroying the IOProc.
    let Some(callback) = (unsafe { context.cast::<Callback>().as_ref() }) else {
        return 0;
    };
    // SAFETY: HAL passes a valid output time for this call.
    let time = unsafe { output_time.as_ref() };
    let entry = Entry {
        at,
        host,
        output_host: time
            .mFlags
            .contains(AudioTimeStampFlags::HostTimeValid)
            .then_some(time.mHostTime),
    };
    // SAFETY: HAL passes this call's output list.
    let buffers = unsafe { hal_buffers(output) };
    // SAFETY: HAL serializes this IOProc's calls, and the buffers are this call's.
    unsafe { block(callback, buffers, entry) };
    0
}

/// The buffers of the output list HAL passed to an IOProc, as one slice.
///
/// # Safety
/// `list` is the output list of the current IOProc call. HAL supplies its whole
/// flexible array of `mNumberBuffers` buffers, writable until the call returns.
pub(crate) unsafe fn hal_buffers<'a>(list: NonNull<AudioBufferList>) -> &'a mut [AudioBuffer] {
    let list = list.as_ptr();
    // SAFETY: the list is valid for this call, and its array starts at `mBuffers`.
    let (count, first) = unsafe {
        (
            (*list).mNumberBuffers as usize,
            std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>(),
        )
    };
    // SAFETY: HAL supplies the entire flexible array, not merely the
    // one-element prefix the SDK struct declares.
    unsafe { std::slice::from_raw_parts_mut(first, count) }
}

/// When HAL entered this block, and its output time.
#[derive(Clone, Copy)]
struct Entry {
    at: Instant,
    /// The host clock at `at`, in HAL's time stamp units.
    host: u64,
    /// When the device reaches this block, or `None` when HAL marked it invalid.
    output_host: Option<u64>,
}

impl Entry {
    /// An entry without an output time, as owned test buffers have.
    #[cfg(test)]
    fn untimed(at: Instant) -> Self {
        Self {
            at,
            host: 0,
            output_host: None,
        }
    }
}

/// This function also runs with owned test buffers; no HAL calls occur here.
///
/// # Safety
/// As for [`block`].
#[cfg(test)]
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
unsafe fn fill(callback: &Callback, buffers: &mut [AudioBuffer], entered: Instant) {
    // SAFETY: the caller's contract is `block`'s.
    unsafe { block(callback, buffers, Entry::untimed(entered)) };
}

/// One HAL block. A panic in its body leaves this and every later block
/// silent, and the control thread reports it (ADR 0238).
///
/// # Safety
/// Calls for one `callback` never overlap: HAL serializes its IOProc, and a
/// test calls this in place of HAL. Each non-null buffer is writable for its
/// `mDataByteSize` bytes during the call.
unsafe fn block(callback: &Callback, buffers: &mut [AudioBuffer], entry: Entry) {
    let finished = callback
        .panic
        // SAFETY: the caller's contract for `block` is the body's.
        .run(|| unsafe { fill_with_time(callback, buffers, entry) });
    if !finished {
        callback.failed.store(true, Ordering::Release);
        // SAFETY: the same buffers the body was given; it may have written part of them.
        unsafe { clear_buffers(buffers) };
    }
}

/// # Safety
/// As for [`block`].
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
unsafe fn fill_with_time(callback: &Callback, buffers: &mut [AudioBuffer], entry: Entry) {
    let _guard = ForbidAlloc::enter();
    let entered = entry.at;
    // SAFETY: only this serialized callback touches the render state, and this shared borrow ends
    // with the statement.
    let was_live = unsafe { &*callback.render.get() }.player.has_live_output();
    // SAFETY: the caller's contract is `fill_inner`'s.
    let deadline = unsafe { fill_inner(callback, buffers, entry) };
    // SAFETY: only this serialized callback touches the trace writer, and no other borrow of it is
    // live.
    let Some(trace) = (unsafe { &mut *callback.trace.get() }).as_mut() else {
        return;
    };
    // SAFETY: only this serialized callback touches the render state, and `fill_inner`'s mutable
    // borrow has ended.
    let state = unsafe { &*callback.render.get() };
    let frames = trace_frames(buffers);
    let revision = callback.transport_revision.load(Ordering::Acquire);
    let authorized = callback.cancellation.is_current(revision);
    trace.record(
        crate::trace::Block {
            frames: frames.unwrap_or(0),
            sample: state.player.current_sample(),
            render_ns: entered.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
            output_host_time: entry.output_host.unwrap_or(0),
            deadline_ns: deadline.map_or(0, |deadline| {
                deadline
                    .saturating_duration_since(entered)
                    .as_nanos()
                    .min(u128::from(u64::MAX)) as u64
            }),
            transport_revision: revision,
            output_revision: state.player.output_revision(),
            authorized,
            stop_fade_offset: state.player.stop_fade_offset(),
            hardware_changed: callback.hardware_epoch.load(Ordering::Acquire)
                != callback.observed_epoch.load(Ordering::Acquire),
            invalid_buffer: frames.is_none(),
            callback_failed: callback.failed.load(Ordering::Acquire),
            engine_failed: state.player.failed(),
            plugin_missed_blocks: state.player.plugin_missed_blocks(),
            cleanup_pending: state.player.cleanup_pending(),
            ..Default::default()
        },
        entered,
        (was_live && !authorized) || state.player.stop_fade_offset() == Some(0),
        // SAFETY: `record` calls this only for frames below `block.frames`, which is zero unless
        // `trace_frames` accepted every buffer's float layout.
        |frame| unsafe { submitted_pair(buffers, frame, callback.channels == 1) },
    );
}

fn trace_frames(buffers: &[AudioBuffer]) -> Option<usize> {
    let mut frames = None;
    for buffer in buffers {
        if buffer.mData.is_null() || buffer.mDataByteSize == 0 {
            continue;
        }
        let bytes = (buffer.mNumberChannels as usize).checked_mul(size_of::<f32>())?;
        if bytes == 0
            || !(buffer.mDataByteSize as usize).is_multiple_of(bytes)
            || !(buffer.mData as usize).is_multiple_of(align_of::<f32>())
        {
            return None;
        }
        let count = buffer.mDataByteSize as usize / bytes;
        if frames.is_some_and(|previous| previous != count) {
            return None;
        }
        frames = Some(count);
    }
    frames
}

/// One frame of the first two channels the block submitted.
///
/// # Safety
/// [`trace_frames`] accepted these buffers, which stay readable for the call.
unsafe fn submitted_pair(buffers: &[AudioBuffer], frame: usize, mono: bool) -> [f32; 2] {
    let mut pair = [0.0; 2];
    let mut base = 0usize;
    for buffer in buffers {
        let channels = buffer.mNumberChannels as usize;
        if !buffer.mData.is_null() && buffer.mDataByteSize != 0 {
            // SAFETY: `trace_frames` found this buffer's data aligned for `f32` and its size a
            // whole number of frames, and HAL keeps it readable during the call.
            let data = unsafe {
                std::slice::from_raw_parts(
                    buffer.mData.cast::<f32>(),
                    buffer.mDataByteSize as usize / size_of::<f32>(),
                )
            };
            for channel in 0..channels.min(2usize.saturating_sub(base)) {
                if let Some(destination) = pair.get_mut(base + channel) {
                    *destination = data.get(frame * channels + channel).copied().unwrap_or(0.0);
                }
            }
        }
        base = base.saturating_add(channels);
        if base >= 2 {
            break;
        }
    }
    if mono {
        [pair.first().copied().unwrap_or(0.0); 2]
    } else {
        pair
    }
}

/// Zeroes every HAL buffer and returns the shared frame count, with false
/// when any buffer is not native float frames of that count.
///
/// # Safety
/// Each non-null buffer points to `mDataByteSize` writable bytes.
pub(crate) unsafe fn clear_buffers(buffers: &mut [AudioBuffer]) -> (Option<usize>, bool) {
    let mut frames = None;
    let mut valid = true;
    for buffer in buffers.iter_mut() {
        if buffer.mData.is_null() || buffer.mDataByteSize == 0 {
            continue;
        }
        // SAFETY: the caller promises `mDataByteSize` writable bytes at each non-null buffer.
        unsafe {
            std::ptr::write_bytes(buffer.mData.cast::<u8>(), 0, buffer.mDataByteSize as usize)
        };
        let channels = buffer.mNumberChannels as usize;
        let bytes_per_frame = channels.saturating_mul(size_of::<f32>());
        if bytes_per_frame == 0
            || !(buffer.mDataByteSize as usize).is_multiple_of(bytes_per_frame)
            || !(buffer.mData as usize).is_multiple_of(align_of::<f32>())
        {
            valid = false;
            continue;
        }
        let size = buffer.mDataByteSize as usize / bytes_per_frame;
        if frames.is_some_and(|frames| frames != size) {
            valid = false;
        }
        frames = Some(size);
    }
    (frames, valid)
}

/// Writes the stereo planes to the device's channels. A mono device gets the
/// equal-power sum; channels after the first two stay silent.
///
/// # Safety
/// [`clear_buffers`] accepted these buffers for at least `left.len()` frames.
pub(crate) unsafe fn write_planes(
    buffers: &mut [AudioBuffer],
    left: &[f32],
    right: &[f32],
    mono: bool,
) {
    let mut channel_base = 0usize;
    for buffer in buffers {
        let channels = buffer.mNumberChannels as usize;
        if channels == 0 {
            continue;
        }
        if !buffer.mData.is_null() && buffer.mDataByteSize != 0 {
            // SAFETY: `clear_buffers` found this buffer's data aligned for `f32` and its size a
            // whole number of frames, and the caller promises it is writable.
            let data = unsafe {
                std::slice::from_raw_parts_mut(
                    buffer.mData.cast::<f32>(),
                    buffer.mDataByteSize as usize / size_of::<f32>(),
                )
            };
            for ((frame, left), right) in data
                .chunks_exact_mut(channels)
                .zip(left.iter())
                .zip(right.iter())
            {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = if mono {
                        (*left + *right) * std::f32::consts::FRAC_1_SQRT_2
                    } else {
                        match channel_base + channel {
                            0 => *left,
                            1 => *right,
                            _ => 0.0,
                        }
                    };
                }
            }
        }
        channel_base = channel_base.saturating_add(channels);
    }
}

/// Renders one block and returns the deadline it gave the player, or `None`
/// when the block was left silent before rendering.
///
/// # Safety
/// As for [`block`].
unsafe fn fill_inner(
    callback: &Callback,
    buffers: &mut [AudioBuffer],
    entry: Entry,
) -> Option<Instant> {
    // SAFETY: the caller's contract is `clear_buffers`'s.
    let (frames, valid) = unsafe { clear_buffers(buffers) };
    // Notifications never query HAL here. Leave bytes zeroed and hold the
    // clock until the control thread has checked the notified configuration.
    if callback.hardware_epoch.load(Ordering::Acquire)
        != callback.observed_epoch.load(Ordering::Acquire)
    {
        return None;
    }
    let frames = frames?;
    if frames == 0 {
        return None;
    }
    // SAFETY: only this serialized callback touches the render state, and no other borrow of it is
    // live.
    let state = unsafe { &mut *callback.render.get() };
    let (Some(left), Some(right)) = (state.left.get_mut(..frames), state.right.get_mut(..frames))
    else {
        callback.failed.store(true, Ordering::Release);
        return None;
    };
    if !valid {
        callback.failed.store(true, Ordering::Release);
        return None;
    }
    let deadline = callback
        .cycle
        .deadline(entry.at, entry.host, entry.output_host, frames);
    let revision = callback.transport_revision.load(Ordering::Acquire);
    let enabled = callback.cancellation.is_current(revision);
    let had_output = state.player.has_live_output();
    let start_sample = state.player.current_sample();
    state
        .player
        .render_live_gated_until(left, right, deadline, enabled);
    callback.callbacks.fetch_add(1, Ordering::Relaxed);
    // An unheard start is cancelled even if Stop arrived during its render.
    // Sounding playback stops at the next boundary, preserving its waveform.
    let allowed = callback.cancellation.is_current(revision);
    if enabled && !allowed && !had_output {
        left.fill(0.0);
        right.fill(0.0);
        state.player.cancel_unheard_start(start_sample);
    }
    let output_revision = state.player.output_revision();
    // An already sounding in-flight block completes. The next boundary releases
    // notes and fades score audio before plugins. An unheard start is suppressed.
    let effective = enabled && (allowed || had_output);
    let changed = effective && (output_revision != state.output_revision || !state.enabled);
    // Stop fades the score source before plugins. A master fade would also
    // cut their releases, delays and keyboard output while stopped.
    state.declick.process(left, right, changed);
    state.output_revision = output_revision;
    state.enabled = effective;
    if !callback.silent {
        // SAFETY: `clear_buffers` accepted these buffers (`valid`) for `frames` frames, which
        // `left` and `right` hold.
        unsafe { write_planes(buffers, left, right, callback.channels == 1) };
    }
    Some(deadline)
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
