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
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use echode_engine::{
    ClipLoader, ForbidAlloc, PlaybackAudio, PlaybackCommand, PlaybackControl, PlaybackStatus,
    PluginActivity, PluginControl, PluginRoute, PluginWorkgroup, playback_pair_at,
    playback_with_plugins_at, playback_with_plugins_in_workgroup_at, tick_to_sample,
};
use echode_model::{Project, Tick};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

use crate::{OutputDevice, TransportCancellation, hardware};

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
    callbacks: AtomicU64,
    trace: UnsafeCell<Option<crate::trace::Writer>>,
    channels: u32,
    /// Prepared output rate. The callback does not ask the device.
    sample_rate: u32,
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
    pub(crate) fader_preview: Option<(u128, f32, f32)>,
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
    pub(crate) fn hal(device: OutputDevice) -> Result<Self, String> {
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
    ) -> Result<Self, String> {
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
    pub(crate) fn preview_fader(&mut self, track: u128, gain: f32, pan: f32) -> Result<(), String> {
        self.control
            .preview_fader(track, gain, pan)
            .map_err(|error| error.to_string())
    }
    pub(crate) fn clear_fader_preview(&mut self, committed: bool) -> Result<(), String> {
        self.control
            .clear_fader_preview(committed)
            .map_err(|error| error.to_string())
    }
    pub(crate) fn meters(&self) -> Vec<Arc<echode_engine::TrackMeter>> {
        self.control.meters()
    }
    pub fn open(device: OutputDevice, project: &Project, tick: i64) -> Result<Self, String> {
        Self::open_mode(device, project, tick, false)
    }

    /// Opens on this thread, helpers included.
    fn open_mode(
        device: OutputDevice,
        project: &Project,
        tick: i64,
        silent: bool,
    ) -> Result<Self, String> {
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
    pub(crate) fn start(claim: Claim, prepared: Prepared, start: Start) -> Result<Self, String> {
        let mut value = Self::assemble(claim, prepared, start);
        value.register()?;
        value.prepare_in_background();
        Ok(value)
    }

    /// Starts HAL's callback. Commands sent before this reach its first block.
    pub(crate) fn register(&mut self) -> Result<(), String> {
        if self.registered {
            return Ok(());
        }
        let mut proc = None;
        let Some(callback) = self.callback.as_mut() else {
            return Err("the output callback is missing".into());
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
            return Err(format!("could not open the output device ({status})"));
        }
        self.proc = proc;
        self.registered = true;
        let status = unsafe { AudioDeviceStart(self.device.id, self.proc) };
        if status != 0 {
            return Err(format!("could not start the output device ({status})"));
        }
        Ok(())
    }

    /// Initial installation has sent all changes before callbacks can start.
    /// Later replacement helpers may prepare while this output keeps running.
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

    /// While `enabled`, a replacement needing new media decodes and converts it
    /// on another thread (ADR 0147). The session enables it while stopped.
    pub(crate) fn load_media_in_background(&mut self, enabled: bool) {
        self.control.load_media_in_background(enabled);
    }

    /// The newest replacement's media has not reached the callback's queue.
    pub(crate) fn loading_media(&self) -> bool {
        self.control.loading_media()
    }
    /// The stream with its callback built but not registered with HAL.
    pub(crate) fn assemble(claim: Claim, prepared: Prepared, start: Start) -> Self {
        let Claim {
            device, listener, ..
        } = claim;
        let Prepared { control, player } = prepared;
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
            callbacks: AtomicU64::new(0),
            trace: UnsafeCell::new(trace),
            channels: device.channels,
            sample_rate: device.sample_rate,
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
            unsafe { fill(callback, &mut [buffer], Instant::now()) };
        }
        data
    }

    pub fn replace(&mut self, project: &Project) -> Result<(), String> {
        if let Some(host) = echode_plugin_host::host_binary() {
            self.control
                .set_routes(&host, &plugin_routes(project, self.package.as_deref())?);
        }
        self.control
            .replace(project)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn resume(&mut self, revision: u64) -> Result<(), String> {
        self.control
            .command(PlaybackCommand::Play)
            .map_err(|error| error.to_string())?;
        if let Some(callback) = &self.callback {
            callback
                .transport_revision
                .store(revision, Ordering::Release);
        }
        Ok(())
    }

    pub(crate) fn pause(&mut self) -> Result<(), String> {
        self.control
            .command(PlaybackCommand::Pause)
            .map_err(|error| error.to_string())
    }

    pub fn set_loop(&mut self, project: &Project, start: i64, end: i64) -> Result<(), String> {
        if end <= start {
            return Err("loop end must be after its start".into());
        }
        let start = loop_sample(project, self.device.sample_rate, start)?;
        let end = loop_sample(project, self.device.sample_rate, end)?;
        if end <= start {
            return Err("loop end must be after its start".into());
        }
        self.control
            .command(PlaybackCommand::SetLoop { start, end })
            .map_err(|error| error.to_string())
    }

    pub fn clear_loop(&mut self) -> Result<(), String> {
        self.control
            .command(PlaybackCommand::ClearLoop)
            .map_err(|error| error.to_string())
    }

    pub fn set_metronome(&mut self, enabled: bool) -> Result<(), String> {
        self.control
            .command(PlaybackCommand::SetMetronome(enabled))
            .map_err(|error| error.to_string())
    }

    pub fn locate(&mut self, project: &Project, tick: i64) -> Result<(), String> {
        let sample = tick_to_sample(Tick::new(tick), project.tempo(), self.device.sample_rate)
            .ok_or("playback time overflow")?;
        self.control
            .command(PlaybackCommand::Locate(sample))
            .map_err(|error| error.to_string())?;
        self.pending_locate = Some((self.control.command_revision(), tick, sample));
        Ok(())
    }

    /// Stops at `tick` without resetting plugins, for a stop that returns to
    /// the remembered start. Tails continue from where playback stopped.
    pub(crate) fn rewind(&mut self, project: &Project, tick: i64) -> Result<(), String> {
        let sample = tick_to_sample(Tick::new(tick), project.tempo(), self.device.sample_rate)
            .ok_or("playback time overflow")?;
        self.control
            .command(PlaybackCommand::Rewind(sample))
            .map_err(|error| error.to_string())?;
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
    pub fn device_control(&self, device: u128) -> Result<PluginControl, String> {
        self.control
            .device_control_handle(device)
            .map_err(|error| error.to_string())
    }

    /// Brings the helpers of `devices` to the project's recorded plugin states,
    /// for an Undo, Redo or API edit that changed only those hashes.
    pub fn sync_plugin_states(
        &mut self,
        project: &Project,
        devices: &[u128],
    ) -> Result<(), String> {
        let package = self.package.clone();
        let recorded = |device: u128| -> Result<Option<Vec<u8>>, String> {
            let state = project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .find(|item| item.id.raw() == device)
                .and_then(|item| item.state.as_deref());
            match (state, package.as_deref()) {
                (None, _) => Ok(None),
                (Some(hash), Some(package)) => echode_history::read_object(package, hash)
                    .map(Some)
                    .map_err(|error| error.to_string()),
                (Some(_), None) => Err("plugin state has no package".into()),
            }
        };
        let replace = self.control.restore_recorded_states(devices, recorded);
        if self.control.preparing_graph() || replace {
            self.replace(project)
        } else {
            Ok(())
        }
    }

    pub fn service(&mut self) -> Result<(), String> {
        if self.workgroup_changed() {
            return Err("the selected output device workgroup changed".into());
        }
        self.control.service();
        if let Some(files) = &mut self.trace_files {
            files.service(self.control.status());
        }
        if let Some(error) = self.control.take_preparation_error() {
            return Err(error.to_string());
        }
        if self.control.status().failed
            || self
                .callback
                .as_ref()
                .is_some_and(|callback| callback.failed.load(Ordering::Acquire))
        {
            return Err(
                "audio playback stopped because the output buffer or device changed".into(),
            );
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
        if let Some(callback) = self.callback.take()
            && self.registered
        {
            let stopped = unsafe { AudioDeviceStop(self.device.id, self.proc) };
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
) -> Result<Vec<PluginRoute>, String> {
    match package {
        Some(package) => routes_with_saved_state(project, package),
        None => Ok(installed_plugin_routes(project)),
    }
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

/// One route per online CLAP device, in project order, with its saved bytes.
///
/// A hash whose object is missing or does not match is an error. Devices that share a
/// plugin id keep separate bytes so a later device does not reuse the first one's state.
pub fn routes_with_saved_state(
    project: &Project,
    package: &std::path::Path,
) -> Result<Vec<PluginRoute>, String> {
    let installed = installed_plugin_routes(project);
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
            if let Some(hash) = &device.state {
                route.state = Some(
                    echode_history::read_object(package, hash)
                        .map_err(|error| error.to_string())?,
                );
            }
            routes.push(route);
        }
    }
    Ok(routes)
}

fn block_budget(frames: usize, sample_rate: u32) -> Duration {
    if sample_rate == 0 || frames == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(
        u64::try_from(frames)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000_000)
            / u64::from(sample_rate),
    )
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
) -> Result<(PlaybackControl, PlaybackAudio), String> {
    prepare_playback_mode(project, sample_rate, frames, tick, transport, None)
}

fn prepare_playback_mode(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    tick: i64,
    transport: &TransportSetup,
    workgroup: Option<PluginWorkgroup>,
) -> Result<(PlaybackControl, PlaybackAudio), String> {
    let sample = tick_to_sample(Tick::new(tick), project.tempo(), sample_rate)
        .ok_or("playback time overflow")?;
    let (mut control, player) = open_playback(
        project,
        sample_rate,
        frames,
        transport.package.as_deref(),
        sample,
        workgroup,
    )?;
    control.set_clip_loader(Arc::clone(&transport.clips));
    control
        .replace(project)
        .map_err(|error| error.to_string())?;
    control
        .command(PlaybackCommand::Locate(sample))
        .map_err(|error| error.to_string())?;
    if let Some((start, end)) = transport.loop_ticks {
        let start = loop_sample(project, sample_rate, start)?;
        let end = loop_sample(project, sample_rate, end)?;
        if end <= start {
            return Err("loop end must be after its start".into());
        }
        control
            .command(PlaybackCommand::SetLoop { start, end })
            .map_err(|error| error.to_string())?;
    }
    control
        .command(PlaybackCommand::SetMetronome(transport.metronome))
        .map_err(|error| error.to_string())?;
    if let Some((track, gain, pan)) = transport.fader_preview {
        control
            .preview_fader(track, gain, pan)
            .map_err(|error| error.to_string())?;
    }
    if transport.start {
        control
            .command(PlaybackCommand::Play)
            .map_err(|error| error.to_string())?;
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
) -> Result<(PlaybackControl, PlaybackAudio), String> {
    if let Some(workgroup) = workgroup {
        let host = echode_plugin_host::host_binary().unwrap_or_default();
        let routes = if host.as_os_str().is_empty() {
            Vec::new()
        } else {
            plugin_routes(project, package)?
        };
        return playback_with_plugins_in_workgroup_at(
            project,
            sample_rate,
            frames,
            &host,
            &routes,
            sample,
            workgroup,
        )
        .map_err(|error| error.to_string());
    }
    if let Some(host) = echode_plugin_host::host_binary() {
        playback_with_plugins_at(
            project,
            sample_rate,
            frames,
            &host,
            &plugin_routes(project, package)?,
            sample,
        )
    } else {
        playback_pair_at(project, sample_rate, frames, sample)
    }
    .map_err(|error| error.to_string())
}

fn loop_sample(project: &Project, sample_rate: u32, tick: i64) -> Result<i64, String> {
    if tick < 0 {
        return Err("loop end must be after its start".into());
    }
    tick_to_sample(Tick::new(tick), project.tempo(), sample_rate)
        .ok_or("playback time overflow".into())
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
    let entered = Instant::now();
    let Some(callback) = (unsafe { context.cast::<Callback>().as_ref() }) else {
        return 0;
    };
    // SAFETY: HAL supplies the entire flexible buffer array, not merely the
    // one-element prefix represented in the SDK struct.
    let list = output.as_ptr();
    let count = unsafe { (*list).mNumberBuffers } as usize;
    let pointer = unsafe { std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>() };
    let buffers = unsafe { std::slice::from_raw_parts_mut(pointer, count) };
    unsafe { fill_with_time(callback, buffers, entered, output_time.as_ref().mHostTime) };
    0
}

/// This function also runs with owned test buffers; no HAL calls occur here.
#[cfg(test)]
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
unsafe fn fill(callback: &Callback, buffers: &mut [AudioBuffer], entered: Instant) {
    unsafe { fill_with_time(callback, buffers, entered, 0) };
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
unsafe fn fill_with_time(
    callback: &Callback,
    buffers: &mut [AudioBuffer],
    entered: Instant,
    host_time: u64,
) {
    let _guard = ForbidAlloc::enter();
    let was_live = unsafe { &*callback.render.get() }.player.has_live_output();
    unsafe { fill_inner(callback, buffers, entered) };
    let Some(trace) = (unsafe { &mut *callback.trace.get() }).as_mut() else {
        return;
    };
    let state = unsafe { &*callback.render.get() };
    let frames = trace_frames(buffers);
    let revision = callback.transport_revision.load(Ordering::Acquire);
    let authorized = callback.cancellation.is_current(revision);
    trace.record(
        crate::trace::Block {
            frames: frames.unwrap_or(0),
            sample: state.player.current_sample(),
            render_ns: entered.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
            output_host_time: host_time,
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

// SAFETY: trace_frames validated the native float layout and frame count.
unsafe fn submitted_pair(buffers: &[AudioBuffer], frame: usize, mono: bool) -> [f32; 2] {
    let mut pair = [0.0; 2];
    let mut base = 0usize;
    for buffer in buffers {
        let channels = buffer.mNumberChannels as usize;
        if !buffer.mData.is_null() && buffer.mDataByteSize != 0 {
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

unsafe fn fill_inner(callback: &Callback, buffers: &mut [AudioBuffer], entered: Instant) {
    let (frames, valid) = unsafe { clear_buffers(buffers) };
    // Notifications never query HAL here. Leave bytes zeroed and hold the
    // clock until the control thread has checked the notified configuration.
    if callback.hardware_epoch.load(Ordering::Acquire)
        != callback.observed_epoch.load(Ordering::Acquire)
    {
        return;
    }
    let Some(frames) = frames else {
        return;
    };
    if frames == 0 {
        return;
    }
    let state = unsafe { &mut *callback.render.get() };
    let (Some(left), Some(right)) = (state.left.get_mut(..frames), state.right.get_mut(..frames))
    else {
        callback.failed.store(true, Ordering::Release);
        return;
    };
    if !valid {
        callback.failed.store(true, Ordering::Release);
        return;
    }
    let deadline = entered + block_budget(frames, callback.sample_rate);
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
    if callback.silent {
        return;
    }
    unsafe { write_planes(buffers, left, right, callback.channels == 1) };
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
