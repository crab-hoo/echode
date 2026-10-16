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
    PluginRoute, playback_pair_at, playback_with_plugins_at, tick_to_sample,
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
        }
    }
}

pub struct OutputStream {
    pub device: OutputDevice,
    control: PlaybackControl,
    callback: Option<Box<Callback>>,
    proc: AudioDeviceIOProcID,
    package: Option<std::path::PathBuf>,
    pending_locate: Option<(u64, i64, i64)>,
    trace_files: Option<crate::trace::Files>,
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

    fn open_mode(
        device: OutputDevice,
        project: &Project,
        tick: i64,
        silent: bool,
    ) -> Result<Self, String> {
        Self::open_with_epoch(
            device,
            project,
            tick,
            silent,
            TransportSetup::idle(),
            Arc::new(AtomicU64::new(0)),
            0,
        )
    }

    pub(crate) fn open_guarded(
        device: OutputDevice,
        project: &Project,
        tick: i64,
        transport: TransportSetup,
        hardware_epoch: Arc<AtomicU64>,
        observed_epoch: u64,
    ) -> Result<Self, String> {
        Self::open_with_epoch(
            device,
            project,
            tick,
            false,
            transport,
            hardware_epoch,
            observed_epoch,
        )
    }

    fn open_with_epoch(
        device: OutputDevice,
        project: &Project,
        tick: i64,
        silent: bool,
        transport: TransportSetup,
        hardware_epoch: Arc<AtomicU64>,
        observed_epoch: u64,
    ) -> Result<Self, String> {
        hardware::validate_streams(&device)?;
        let frames = device.buffer_frames as usize;
        let (control, player) =
            prepare_playback(project, device.sample_rate, frames, tick, &transport)?;
        let (trace, trace_files) = crate::trace::configured(device.sample_rate, frames)
            .map_or((None, None), |(writer, files)| (Some(writer), Some(files)));
        let mut callback = Box::new(Callback {
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
            silent,
            hardware_epoch,
            observed_epoch: AtomicU64::new(observed_epoch),
            cancellation: transport.cancellation.clone(),
            transport_revision: AtomicU64::new(transport.revision),
        });
        let mut proc = None;
        let context = (&mut *callback as *mut Callback).cast();
        // SAFETY: the boxed context has a stable address and out_proc is SDK typed.
        let status = unsafe {
            AudioDeviceCreateIOProcID(device.id, Some(render), context, NonNull::from(&mut proc))
        };
        if status != 0 {
            return Err(format!("could not open the output device ({status})"));
        }
        let value = Self {
            device,
            control,
            callback: Some(callback),
            proc,
            package: transport.package.clone(),
            pending_locate: None,
            trace_files,
        };
        let status = unsafe { AudioDeviceStart(value.device.id, value.proc) };
        if status != 0 {
            return Err(format!("could not start the output device ({status})"));
        }
        Ok(value)
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

    pub fn take_plugin_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        self.control.take_plugin_saves()
    }

    pub fn service(&mut self) -> Result<(), String> {
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
        if let Some(callback) = self.callback.take() {
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

/// CLAP ids in the order devices first name them. A repeated id is listed once.
pub fn project_plugin_ids(project: &Project) -> Vec<String> {
    let mut ids = Vec::new();
    for track in project.tracks() {
        for device in &track.devices {
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
/// on the built-in synth. Export must refuse the file instead.
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

/// One route per CLAP device, in project order, with saved bytes when the device names them.
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
fn prepare_playback(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    tick: i64,
    transport: &TransportSetup,
) -> Result<(PlaybackControl, PlaybackAudio), String> {
    let sample = tick_to_sample(Tick::new(tick), project.tempo(), sample_rate)
        .ok_or("playback time overflow")?;
    let (mut control, player) = open_playback(
        project,
        sample_rate,
        frames,
        transport.package.as_deref(),
        sample,
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
    control
        .command(PlaybackCommand::Play)
        .map_err(|error| error.to_string())?;
    Ok((control, player))
}

fn open_playback(
    project: &Project,
    sample_rate: u32,
    frames: usize,
    package: Option<&std::path::Path>,
    sample: i64,
) -> Result<(PlaybackControl, PlaybackAudio), String> {
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

unsafe fn fill_inner(callback: &Callback, buffers: &mut [AudioBuffer], entered: Instant) {
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
    // An already sounding in-flight block completes. The next boundary fades
    // held voices without scheduling new notes. An unheard start is suppressed.
    let effective = enabled && (allowed || had_output);
    let changed =
        (effective && output_revision != state.output_revision) || effective != state.enabled;
    if let Some(offset) = state.player.stop_fade_offset() {
        state.declick.process_stop(left, right, offset);
    } else {
        state.declick.process(left, right, changed);
    }
    state.output_revision = output_revision;
    state.enabled = effective;
    if callback.silent {
        return;
    }
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
                    *sample = if callback.channels == 1 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use echode_engine::playback_pair;

    #[test]
    fn delayed_control_pause_preserves_stop_and_trace_matches_native_buffers_under_rtsan() {
        let rate = 48_000;
        let frames = 64; // Device geometry of this fixture, not a product limit.
        for layout in 0..3 {
            for delay in [0, 1, 20] {
                let directory = std::env::temp_dir().join(format!(
                    "echode-stop-native-{}-{layout}-{delay}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&directory).unwrap();
                let (trace, mut files) = crate::trace::pair(rate, frames, directory.clone());
                let (project, track) = Project::new().add_track("stop trace");
                let (mut control, player) = playback_pair(&project, rate, frames).unwrap();
                let wave = |sample: usize, channel: usize| {
                    let (amplitude, frequency) = [(0.25, 437.0), (-0.5, 1601.0)][channel];
                    amplitude
                        * (std::f32::consts::TAU * frequency * sample as f32 / rate as f32 + 0.7)
                            .sin()
                };
                let left: Arc<[f32]> = (0..rate as usize).map(|i| wave(i, 0)).collect();
                let right: Arc<[f32]> = (0..rate as usize).map(|i| wave(i, 1)).collect();
                control.set_clip_loader(Arc::new(move |_, _| {
                    Ok(vec![echode_engine::LoadedClip {
                        track: track.raw(),
                        start_sample: 0,
                        samples: Arc::clone(&left),
                        right: Some(Arc::clone(&right)),
                        fade_in: 0,
                        fade_out: 0,
                    }])
                }));
                control.replace(&project).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let cancellation = TransportCancellation::default();
                let callback = Callback {
                    render: UnsafeCell::new(Render {
                        player,
                        declick: echode_engine::OutputDeclick::new(rate),
                        output_revision: 0,
                        enabled: true,
                        left: vec![0.0; frames],
                        right: vec![0.0; frames],
                    }),
                    failed: AtomicBool::new(false),
                    callbacks: AtomicU64::new(0),
                    trace: UnsafeCell::new(Some(trace)),
                    channels: if layout == 2 { 1 } else { 2 },
                    sample_rate: rate,
                    silent: false,
                    hardware_epoch: Arc::new(AtomicU64::new(0)),
                    observed_epoch: AtomicU64::new(0),
                    cancellation: cancellation.clone(),
                    transport_revision: AtomicU64::new(0),
                };
                let mut interleaved = vec![0.0f32; frames * if layout == 2 { 1 } else { 2 }];
                let mut planar = [vec![0.0f32; frames], vec![0.0f32; frames]];
                let mut history = Vec::<[f32; 2]>::new();
                let mut run = || {
                    let mut buffers: Vec<_> = if layout == 1 {
                        planar
                            .iter_mut()
                            .map(|plane| AudioBuffer {
                                mNumberChannels: 1,
                                mDataByteSize: std::mem::size_of_val(plane.as_slice()) as u32,
                                mData: plane.as_mut_ptr().cast(),
                            })
                            .collect()
                    } else {
                        vec![AudioBuffer {
                            mNumberChannels: if layout == 2 { 1 } else { 2 },
                            mDataByteSize: std::mem::size_of_val(interleaved.as_slice()) as u32,
                            mData: interleaved.as_mut_ptr().cast(),
                        }]
                    };
                    unsafe {
                        fill_with_time(
                            &callback,
                            &mut buffers,
                            Instant::now(),
                            history.len() as u64 + 1,
                        )
                    };
                    for frame in 0..frames {
                        history.push(match layout {
                            1 => [planar[0][frame], planar[1][frame]],
                            2 => [interleaved[frame]; 2],
                            _ => [interleaved[frame * 2], interleaved[frame * 2 + 1]],
                        });
                    }
                };
                for _ in 0..20 {
                    run();
                    files.service(control.status());
                }
                let stopped = control.status().sample;
                cancellation.request(&crate::AudioCommand::Stop);
                for index in 0..60 {
                    if index == delay {
                        control.command(PlaybackCommand::Pause).unwrap();
                    }
                    run();
                    files.service(control.status());
                    assert_eq!(control.status().sample, stopped);
                }
                let fade = (rate / 200) as usize;
                for (age, output) in history[stopped as usize..].iter().enumerate() {
                    let gain = if age >= fade - 1 {
                        0.0
                    } else {
                        0.5 * (1.0 + (std::f64::consts::PI * age as f64 / (fade - 1) as f64).cos())
                            as f32
                    };
                    let mut expected = [
                        wave(stopped as usize + age, 0) * gain,
                        wave(stopped as usize + age, 1) * gain,
                    ];
                    if layout == 2 {
                        expected =
                            [(expected[0] + expected[1]) * std::f32::consts::FRAC_1_SQRT_2; 2];
                    }
                    for channel in 0..2 {
                        assert!(
                            (output[channel] - expected[channel]).abs() < 1.0e-6,
                            "late Pause changed output at {layout}/{delay}/{age}: {} != {}",
                            output[channel],
                            expected[channel]
                        );
                    }
                }
                let captures: Vec<_> = std::fs::read_dir(&directory)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .collect();
                assert_eq!(captures.len(), 1);
                let metadata: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&captures[0]).unwrap()).unwrap();
                assert_eq!(metadata["stop_frame"], stopped as u64);
                assert_eq!(metadata["stop_retained"], true);
                let first = metadata["first_frame"].as_u64().unwrap() as usize;
                let pcm = std::fs::read(directory.join(metadata["pcm"].as_str().unwrap())).unwrap();
                for (index, bytes) in pcm.chunks_exact(8).enumerate() {
                    for channel in 0..2 {
                        let value = u32::from_le_bytes(
                            bytes[channel * 4..channel * 4 + 4].try_into().unwrap(),
                        );
                        assert_eq!(value, history[first + index][channel].to_bits());
                    }
                }
                assert!(!control.status().failed);
                assert!(!callback.failed.load(Ordering::Acquire));
                std::fs::remove_dir_all(directory).unwrap();
            }
        }
    }

    #[test]
    fn contract_transport_discontinuities_declick_hal_output_under_rtsan() {
        // Clip and scratch lengths cover this transport fixture only; they do
        // not constrain product tracks, channels, or device buffer sizes.
        for rate in [48_000, 96_000] {
            for frames in [1, 7, 64, 512] {
                for command in [
                    Some(PlaybackCommand::Stop),
                    Some(PlaybackCommand::Pause),
                    None,
                ] {
                    let cancellation_only = command.is_none();
                    let (project, track) = Project::new().add_track("continuous audio");
                    let (mut control, player) = playback_pair(&project, rate, frames).unwrap();
                    control.set_clip_loader(Arc::new(move |_, _| {
                        Ok(vec![echode_engine::LoadedClip {
                            track: track.raw(),
                            start_sample: 0,
                            samples: vec![0.25; 10_000].into(),
                            right: Some(vec![-0.5; 10_000].into()),
                            fade_in: 0,
                            fade_out: 0,
                        }])
                    }));
                    control.replace(&project).unwrap();
                    control.command(PlaybackCommand::Play).unwrap();
                    let cancellation = TransportCancellation::default();
                    let callback = Callback {
                        render: UnsafeCell::new(Render {
                            player,
                            declick: echode_engine::OutputDeclick::new(rate),
                            output_revision: 0,
                            enabled: true,
                            left: vec![0.0; frames],
                            right: vec![0.0; frames],
                        }),
                        failed: AtomicBool::new(false),
                        callbacks: AtomicU64::new(0),
                        trace: UnsafeCell::new(None),
                        channels: 2,
                        sample_rate: rate,
                        silent: false,
                        hardware_epoch: Arc::new(AtomicU64::new(0)),
                        observed_epoch: AtomicU64::new(0),
                        cancellation: cancellation.clone(),
                        transport_revision: AtomicU64::new(0),
                    };
                    let mut stereo = vec![0.0f32; frames * 2];
                    let mut run = || {
                        let buffer = AudioBuffer {
                            mNumberChannels: 2,
                            mDataByteSize: std::mem::size_of_val(stereo.as_slice()) as u32,
                            mData: stereo.as_mut_ptr().cast(),
                        };
                        unsafe { fill(&callback, &mut [buffer], Instant::now()) };
                        stereo.clone()
                    };
                    let fade = (rate / 200) as usize;
                    for _ in 0..fade.div_ceil(frames) + 1 {
                        run();
                    }
                    let position = control.status().sample;
                    if let Some(command) = command {
                        control.command(command).unwrap();
                    } else {
                        cancellation.request(&crate::AudioCommand::Stop);
                    }
                    let mut output = Vec::new();
                    for _ in 0..fade.div_ceil(frames) + 2 {
                        output.extend(run());
                    }
                    assert_eq!(control.status().sample, position);
                    assert!(!control.status().playing);
                    // The stopped clock must not cut a nonzero waveform to zero.
                    assert_eq!(
                        &output[..2],
                        &[0.25, -0.5],
                        "stop edge {rate}/{frames}/{cancellation_only}"
                    );
                    for (index, pair) in output.chunks_exact(2).enumerate() {
                        let gain = if index >= fade - 1 {
                            0.0
                        } else {
                            0.5 * (1.0
                                + (std::f64::consts::PI * index as f64 / (fade - 1) as f64).cos())
                                as f32
                        };
                        for channel in 0..2 {
                            assert!((pair[channel] - [0.25, -0.5][channel] * gain).abs() < 1.0e-7);
                            if index >= fade - 1 {
                                assert_eq!(pair[channel], 0.0);
                            }
                        }
                    }
                    assert!(!callback.failed.load(Ordering::Acquire));
                }
            }
        }
    }

    #[test]
    fn contract_stop_preserves_waveform_while_fading_under_rtsan() {
        // Rates, phases and buffers bound only this regression fixture.
        for rate in [48_000, 96_000] {
            for frames in [1, 7, 64, 512] {
                for phase in [0.0f32, 0.7, 1.6] {
                    for command in [
                        Some(PlaybackCommand::Stop),
                        Some(PlaybackCommand::Pause),
                        None,
                    ] {
                        let sample = move |index: usize, channel: usize| {
                            let (amplitude, frequency) = [(0.25, 437.0), (0.5, 1601.0)][channel];
                            amplitude
                                * (std::f32::consts::TAU * frequency * index as f32 / rate as f32
                                    + phase)
                                    .sin()
                        };
                        let (project, track) = Project::new().add_track("waveform");
                        let (mut control, player) = playback_pair(&project, rate, frames).unwrap();
                        let length = rate as usize;
                        let pcm_left: Arc<[f32]> = (0..length).map(|i| sample(i, 0)).collect();
                        let pcm_right: Arc<[f32]> = (0..length).map(|i| sample(i, 1)).collect();
                        control.set_clip_loader(Arc::new(move |_, _| {
                            Ok(vec![echode_engine::LoadedClip {
                                track: track.raw(),
                                start_sample: 0,
                                samples: Arc::clone(&pcm_left),
                                right: Some(Arc::clone(&pcm_right)),
                                fade_in: 0,
                                fade_out: 0,
                            }])
                        }));
                        control.replace(&project).unwrap();
                        control.command(PlaybackCommand::Play).unwrap();
                        let cancellation = TransportCancellation::default();
                        let callback = Callback {
                            render: UnsafeCell::new(Render {
                                player,
                                declick: echode_engine::OutputDeclick::new(rate),
                                output_revision: 0,
                                enabled: true,
                                left: vec![0.0; frames],
                                right: vec![0.0; frames],
                            }),
                            failed: AtomicBool::new(false),
                            callbacks: AtomicU64::new(0),
                            trace: UnsafeCell::new(None),
                            channels: 2,
                            sample_rate: rate,
                            silent: false,
                            hardware_epoch: Arc::new(AtomicU64::new(0)),
                            observed_epoch: AtomicU64::new(0),
                            cancellation: cancellation.clone(),
                            transport_revision: AtomicU64::new(0),
                        };
                        let mut output = vec![0.0f32; frames * 2];
                        let mut run = || {
                            let buffer = AudioBuffer {
                                mNumberChannels: 2,
                                mDataByteSize: std::mem::size_of_val(output.as_slice()) as u32,
                                mData: output.as_mut_ptr().cast(),
                            };
                            unsafe {
                                fill(&callback, &mut [buffer], Instant::now());
                            }
                            output.clone()
                        };
                        let fade = (rate / 200) as usize;
                        for _ in 0..(fade * 3).div_ceil(frames) {
                            run();
                        }
                        let stopped = control.status().sample as usize;
                        if let Some(command) = command {
                            control.command(command).unwrap();
                        } else {
                            cancellation.request(&crate::AudioCommand::Stop);
                        }
                        let mut heard = Vec::new();
                        for _ in 0..fade.div_ceil(frames) + 2 {
                            heard.extend(run());
                        }
                        for (offset, pair) in heard.chunks_exact(2).enumerate() {
                            let gain = if offset >= fade - 1 {
                                0.0
                            } else {
                                0.5 * (1.0
                                    + (std::f64::consts::PI * offset as f64 / (fade - 1) as f64)
                                        .cos()) as f32
                            };
                            for (channel, value) in pair.iter().enumerate() {
                                let expected = sample(stopped + offset, channel) * gain;
                                assert!(
                                    (*value - expected).abs() < 2.0e-6,
                                    "stop lost waveform at {rate}/{frames}, phase {phase}, offset {offset}, channel {channel}: {} != {expected}",
                                    pair[channel]
                                );
                            }
                        }
                        assert_eq!(control.status().sample as usize, stopped);
                        assert!(!control.status().playing);
                        assert!(!callback.failed.load(Ordering::Acquire));
                    }
                }
            }
        }
    }

    #[test]
    fn in_flight_stop_preserves_heard_audio_and_cancels_unheard_starts_under_rtsan() {
        struct Clock {
            sample: i64,
            armed: Arc<AtomicBool>,
            cancellation: TransportCancellation,
        }
        impl echode_engine::ClockSource for Clock {
            fn position(&self) -> i64 {
                self.sample
            }
            fn locate(&mut self, sample: i64) {
                self.sample = sample;
                if self.armed.swap(false, Ordering::AcqRel) {
                    self.cancellation.request(&crate::AudioCommand::Stop);
                }
            }
            fn advance(&mut self, frames: u32) -> Result<(), echode_engine::PlaybackError> {
                self.locate(self.sample + i64::from(frames));
                Ok(())
            }
        }
        for already_heard in [false, true] {
            let rate = 48_000;
            let frames = 64; // Fixture buffer size only.
            let cancellation = TransportCancellation::default();
            let armed = Arc::new(AtomicBool::new(false));
            let (project, track) = Project::new().add_track("continuing PCM");
            let (mut control, player) = echode_engine::playback_with_clock(
                &project,
                rate,
                frames,
                Box::new(Clock {
                    sample: 0,
                    armed: Arc::clone(&armed),
                    cancellation: cancellation.clone(),
                }),
            )
            .unwrap();
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![echode_engine::LoadedClip {
                    track: track.raw(),
                    start_sample: 0,
                    samples: vec![0.25; 4096].into(),
                    right: Some(vec![-0.5; 4096].into()),
                    fade_in: 0,
                    fade_out: 0,
                }])
            }));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let callback = Callback {
                render: UnsafeCell::new(Render {
                    player,
                    declick: echode_engine::OutputDeclick::new(rate),
                    output_revision: 0,
                    enabled: true,
                    left: vec![0.0; frames],
                    right: vec![0.0; frames],
                }),
                failed: AtomicBool::new(false),
                callbacks: AtomicU64::new(0),
                trace: UnsafeCell::new(None),
                channels: 2,
                sample_rate: rate,
                silent: false,
                hardware_epoch: Arc::new(AtomicU64::new(0)),
                observed_epoch: AtomicU64::new(0),
                cancellation,
                transport_revision: AtomicU64::new(0),
            };
            let mut output = vec![0.0f32; frames * 2];
            let mut run = || {
                let buffer = AudioBuffer {
                    mNumberChannels: 2,
                    mDataByteSize: std::mem::size_of_val(output.as_slice()) as u32,
                    mData: output.as_mut_ptr().cast(),
                };
                unsafe {
                    fill(&callback, &mut [buffer], Instant::now());
                }
                output.clone()
            };
            if already_heard {
                for _ in 0..8 {
                    run();
                }
            }
            let before = control.status().sample;
            armed.store(true, Ordering::Release);
            let heard = run();
            if already_heard {
                assert!(
                    heard.chunks_exact(2).all(|pair| pair == [0.25, -0.5]),
                    "in-flight output: {:?}, status: {:?}",
                    &heard[..8],
                    control.status()
                );
                assert_eq!(control.status().sample, before + frames as i64);
            } else {
                assert!(heard.iter().all(|sample| *sample == 0.0));
                assert_eq!(control.status().sample, before);
            }
            let stopped = control.status().sample;
            for _ in 0..8 {
                run();
                assert_eq!(control.status().sample, stopped);
            }
            assert!(!control.status().playing);
            assert!(run().iter().all(|sample| *sample == 0.0));
            assert!(!callback.failed.load(Ordering::Acquire));
        }
    }

    #[test]
    fn retained_stream_publishes_a_stopped_locate_before_its_callback() {
        let project = Project::new();
        let (control, mut player) =
            prepare_playback(&project, 48000, 64, 0, &TransportSetup::idle()).unwrap();
        let mut stream = OutputStream {
            device: OutputDevice {
                id: 0,
                uid: "fixture".into(),
                name: "fixture".into(),
                sample_rate: 48000,
                buffer_frames: 64,
                channels: 2,
                formats: Vec::new(),
            },
            control,
            callback: None,
            proc: None,
            package: None,
            pending_locate: None,
            trace_files: None,
        };
        stream.pause().unwrap();
        stream.locate(&project, 705_600 * 2).unwrap();
        assert_eq!(stream.status().tick, 705_600 * 2);
        // Another command may be consumed in the same callback as Locate.
        stream.resume(0).unwrap();
        let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
        player.render_gated_until(
            &mut left,
            &mut right,
            Instant::now() + Duration::from_secs(1),
            false,
        );
        assert_eq!(stream.status().tick, 705_600 * 2);
        stream.resume(0).unwrap();
        player.render(&mut left, &mut right);
        assert_eq!(stream.status().sample, 48000 + 64);
    }

    #[test]
    fn contract_stop_during_preparation_gates_callbacks_under_rtsan() {
        let (project, track) = Project::new().add_track("pad");
        let project = project
            .insert_note(
                track,
                echode_model::NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                },
            )
            .unwrap()
            .0;
        let cancellation = TransportCancellation::default();
        let mut setup = TransportSetup::idle();
        setup.cancellation = cancellation.clone();
        setup.revision = cancellation.request(&crate::AudioCommand::Play);
        let interrupt = cancellation.clone();
        // The product media loader is called during preparation, before HAL
        // can start. Submit Stop from that phase, without relying on timing.
        setup.clips = Arc::new(move |_, _| {
            interrupt.request(&crate::AudioCommand::Stop);
            Ok(Vec::new())
        });
        let (mut control, player) = prepare_playback(&project, 48000, 64, 0, &setup).unwrap();
        let callback = Callback {
            render: UnsafeCell::new(Render {
                player,
                declick: echode_engine::OutputDeclick::new(48000),
                output_revision: 0,
                enabled: true,
                left: vec![0.0; 64],
                right: vec![0.0; 64],
            }),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            trace: UnsafeCell::new(None),
            channels: 2,
            sample_rate: 48000,
            silent: false,
            hardware_epoch: Arc::new(AtomicU64::new(0)),
            observed_epoch: AtomicU64::new(0),
            cancellation: cancellation.clone(),
            transport_revision: AtomicU64::new(setup.revision),
        };
        // Fixed storage covers only this 64-frame stereo HAL fixture.
        let mut stereo = [1.0f32; 128];
        let mut run = || {
            let buffer = AudioBuffer {
                mNumberChannels: 2,
                mDataByteSize: std::mem::size_of_val(&stereo) as u32,
                mData: stereo.as_mut_ptr().cast(),
            };
            unsafe { fill(&callback, &mut [buffer], Instant::now()) };
            stereo
        };
        assert!(run().iter().all(|sample| *sample == 0.0));
        assert_eq!(control.status().sample, 0);
        assert!(!control.status().playing);
        // A fresh Play reuses the prepared player. Stop gates it immediately,
        // even if the control worker has not consumed the queued stop yet.
        let revision = cancellation.request(&crate::AudioCommand::Play);
        control.command(PlaybackCommand::Play).unwrap();
        callback
            .transport_revision
            .store(revision, Ordering::Release);
        assert!(run().iter().any(|sample| *sample != 0.0));
        let held = control.status().sample;
        cancellation.request(&crate::AudioCommand::Stop);
        for block in 0..5 {
            let output = run();
            if block == 4 {
                assert!(output.iter().all(|sample| *sample == 0.0));
            }
            assert_eq!(control.status().sample, held);
        }
        assert!(!callback.failed.load(Ordering::Acquire));
    }
    use std::time::{Duration, Instant};

    #[test]
    fn preview_before_open_applies_before_any_live_output_under_rtsan() {
        let (project, track) = Project::new().add_track("preview before play");
        let project = project
            .insert_note(
                track,
                echode_model::NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                },
            )
            .unwrap()
            .0;
        for rate in [48_000, 96_000] {
            let mut setup = TransportSetup::idle();
            setup.fader_preview = Some((track.raw(), 0.0, 0.0));
            let (mut control, mut player) =
                prepare_playback(&project, rate, 64, 0, &setup).unwrap();
            // These buffers cover only this product preparation fixture.
            let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
            for _ in 0..16 {
                player.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
                assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
            }
            assert_eq!(control.status().sample, 1024);
            control.clear_fader_preview(false).unwrap();
            for _ in 0..16 {
                player.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
            }
            assert!(left.iter().chain(&right).any(|sample| *sample != 0.0));
            assert!(!control.status().failed);
        }
    }

    #[test]
    fn contract_startup_position_is_published_before_the_first_callback() {
        let project = Project::new();
        // Rates and scratch lengths are this regression's cases, not product limits.
        for rate in [48_000, 96_000] {
            for frames in [1, 64, 512] {
                for beat in [2, 7] {
                    let tick = beat * echode_model::TICKS_PER_QUARTER;
                    let sample = tick_to_sample(Tick::new(tick), project.tempo(), rate).unwrap();
                    let (control, mut player) =
                        prepare_playback(&project, rate, frames, tick, &TransportSetup::idle())
                            .unwrap();
                    assert_eq!(
                        control.status().tick,
                        tick,
                        "a newly opened stream published zero before its first callback"
                    );
                    assert_eq!(control.status().sample, sample);
                    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                    player.render(&mut left, &mut right);
                    assert_eq!(control.status().sample, sample + frames as i64);
                    assert!(!control.status().failed);
                }
            }
        }
    }

    #[test]
    fn interleaved_and_planar_hal_buffers_match_under_rtsan() {
        let (project, track) = Project::new().add_track("pad");
        let project = project
            .insert_note(
                track,
                echode_model::NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                },
            )
            .unwrap()
            .0;
        let callback = || {
            let (mut control, player) = playback_pair(&project, 48000, 64).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            (
                control,
                Callback {
                    render: UnsafeCell::new(Render {
                        player,
                        declick: echode_engine::OutputDeclick::new(48000),
                        output_revision: 0,
                        enabled: true,
                        left: vec![0.0; 64],
                        right: vec![0.0; 64],
                    }),
                    failed: AtomicBool::new(false),
                    callbacks: AtomicU64::new(0),
                    trace: UnsafeCell::new(None),
                    channels: 2,
                    sample_rate: 48_000,
                    silent: false,
                    hardware_epoch: Arc::new(AtomicU64::new(0)),
                    observed_epoch: AtomicU64::new(0),
                    cancellation: TransportCancellation::default(),
                    transport_revision: AtomicU64::new(0),
                },
            )
        };
        let (control_a, a) = callback();
        let (control_b, b) = callback();
        let (mut stereo, mut left, mut right, mut extra) =
            ([0.0f32; 128], [0.0f32; 64], [0.0f32; 64], [1.0f32; 64]);
        let buffer = |data: &mut [f32], channels| AudioBuffer {
            mNumberChannels: channels,
            mDataByteSize: std::mem::size_of_val(data) as u32,
            mData: data.as_mut_ptr().cast(),
        };
        unsafe {
            fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now());
        }
        unsafe {
            fill(
                &b,
                &mut [
                    buffer(&mut left, 1),
                    buffer(&mut right, 1),
                    buffer(&mut extra, 1),
                ],
                Instant::now(),
            );
        }
        for (frame, (left, right)) in stereo.chunks_exact(2).zip(left.iter().zip(&right)) {
            assert_eq!(frame, &[*left, *right]);
        }
        assert!(extra.iter().all(|sample| *sample == 0.0));
        assert!(stereo.iter().any(|sample| *sample != 0.0));
        assert_eq!(control_a.status().sample, control_b.status().sample);
        let held = control_a.status().sample;
        a.hardware_epoch.fetch_add(1, Ordering::Release);
        stereo.fill(1.0);
        unsafe { fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now()) };
        assert!(stereo.iter().all(|sample| *sample == 0.0));
        assert_eq!(control_a.status().sample, held);
        a.observed_epoch.store(1, Ordering::Release);
        unsafe { fill(&a, &mut [buffer(&mut stereo, 2)], Instant::now()) };
        assert_eq!(control_a.status().sample, held + 64);
        assert!(stereo.iter().any(|sample| *sample != 0.0));
        assert!(!a.failed.load(Ordering::Acquire) && !b.failed.load(Ordering::Acquire));
        // A driver enlargement beyond prepared scratch is silent, never a panic.
        let mut large = [1.0f32; 256];
        unsafe {
            fill(&a, &mut [buffer(&mut large, 2)], Instant::now());
        }
        assert!(large.iter().all(|sample| *sample == 0.0));
        assert!(a.failed.load(Ordering::Acquire));
    }

    #[test]
    fn the_hal_deadline_starts_when_the_callback_is_entered() {
        let entered = Instant::now();
        std::thread::sleep(Duration::from_millis(30));
        let spent = entered + block_budget(64, 48_000);
        let restarted = Instant::now() + block_budget(64, 48_000);
        assert!(spent < Instant::now());
        assert!(restarted > spent + Duration::from_millis(20));
        let (project, track) = Project::new().add_track("pad");
        let project = project
            .insert_note(
                track,
                echode_model::NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                },
            )
            .unwrap()
            .0;
        let (mut control, player) = playback_pair(&project, 48_000, 64).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let callback = Callback {
            render: UnsafeCell::new(Render {
                player,
                declick: echode_engine::OutputDeclick::new(48000),
                output_revision: 0,
                enabled: true,
                left: vec![0.0; 64],
                right: vec![0.0; 64],
            }),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            trace: UnsafeCell::new(None),
            channels: 2,
            sample_rate: 48_000,
            silent: false,
            hardware_epoch: Arc::new(AtomicU64::new(0)),
            observed_epoch: AtomicU64::new(0),
            cancellation: TransportCancellation::default(),
            transport_revision: AtomicU64::new(0),
        };
        let mut stereo = [0.0f32; 128];
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: std::mem::size_of_val(&stereo) as u32,
            mData: stereo.as_mut_ptr().cast(),
        };
        unsafe { fill(&callback, &mut [buffer], entered) };
        assert!(stereo.iter().any(|sample| *sample != 0.0));
        assert!(!callback.failed.load(Ordering::Acquire));
    }

    #[test]
    #[ignore = "needs a CoreAudio output; make spike-audio runs the silent probe"]
    fn a_live_output_runs_silently_and_stops_repeatedly() {
        let inventory = crate::inventory().expect("output inventory");
        let device = inventory
            .devices
            .iter()
            .find(|device| device.id == inventory.default_device)
            .expect("default output");
        for _ in 0..4 {
            let mut stream = OutputStream::open_mode(device.clone(), &Project::new(), 0, true)
                .expect("open output");
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                stream.service().unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(stream.callbacks() > 0);
            let status = stream.close();
            assert!(status.sample > 0 && !status.failed);
        }
    }
}
