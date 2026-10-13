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

use echode_engine::{
    ClipLoader, ForbidAlloc, PlaybackAudio, PlaybackCommand, PlaybackControl, PlaybackStatus,
    PluginRoute, playback_pair, playback_with_plugins, tick_to_sample,
};
use echode_model::{Project, Tick};
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

use crate::{OutputDevice, hardware};

struct Render {
    player: PlaybackAudio,
    left: Vec<f32>,
    right: Vec<f32>,
}

struct Callback {
    render: UnsafeCell<Render>,
    failed: AtomicBool,
    callbacks: AtomicU64,
    channels: u32,
    silent: bool,
    hardware_epoch: Arc<AtomicU64>,
    observed_epoch: AtomicU64,
}

// SAFETY: HAL serializes calls to this IOProc. Only that callback touches
// render; the control thread frees it after stopping/destroying the IOProc.
unsafe impl Sync for Callback {}

pub(crate) struct TransportSetup {
    pub(crate) loop_ticks: Option<(i64, i64)>,
    pub(crate) metronome: bool,
    pub(crate) clips: ClipLoader,
}

impl TransportSetup {
    fn idle() -> Self {
        Self {
            loop_ticks: None,
            metronome: false,
            clips: Arc::new(|_project, _rate| Ok(Vec::new())),
        }
    }
}

pub struct OutputStream {
    pub device: OutputDevice,
    control: PlaybackControl,
    callback: Option<Box<Callback>>,
    proc: AudioDeviceIOProcID,
}

impl OutputStream {
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
        let sample = tick_to_sample(Tick::new(tick), project.tempo(), device.sample_rate)
            .ok_or("playback time overflow")?;
        let frames = device.buffer_frames as usize;
        let (mut control, player) = open_playback(project, device.sample_rate, frames)?;
        control.set_clip_loader(transport.clips);
        control
            .replace(project)
            .map_err(|error| error.to_string())?;
        control
            .command(PlaybackCommand::Locate(sample))
            .map_err(|error| error.to_string())?;
        if let Some((start, end)) = transport.loop_ticks {
            let start = loop_sample(project, device.sample_rate, start)?;
            let end = loop_sample(project, device.sample_rate, end)?;
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
        control
            .command(PlaybackCommand::Play)
            .map_err(|error| error.to_string())?;
        let mut callback = Box::new(Callback {
            render: UnsafeCell::new(Render {
                player,
                left: vec![0.0; frames],
                right: vec![0.0; frames],
            }),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            channels: device.channels,
            silent,
            hardware_epoch,
            observed_epoch: AtomicU64::new(observed_epoch),
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
        };
        let status = unsafe { AudioDeviceStart(value.device.id, value.proc) };
        if status != 0 {
            return Err(format!("could not start the output device ({status})"));
        }
        Ok(value)
    }

    pub fn replace(&mut self, project: &Project) -> Result<(), String> {
        if let Some(host) = echode_plugin_host::host_binary() {
            self.control.set_routes(&host, &plugin_routes(project));
        }
        self.control
            .replace(project)
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
            .map_err(|error| error.to_string())
    }

    pub fn service(&mut self) -> Result<(), String> {
        self.control.service();
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
        self.control.status()
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
        let mut status = self.control.status();
        status.failed |= failed;
        status
    }
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        self.close();
    }
}

fn plugin_routes(project: &Project) -> Vec<PluginRoute> {
    installed_plugin_routes(project)
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
        })
        .collect()
}

fn open_playback(
    project: &Project,
    sample_rate: u32,
    frames: usize,
) -> Result<(PlaybackControl, PlaybackAudio), String> {
    if let Some(host) = echode_plugin_host::host_binary() {
        playback_with_plugins(project, sample_rate, frames, &host, &plugin_routes(project))
    } else {
        playback_pair(project, sample_rate, frames)
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
    _: NonNull<AudioTimeStamp>,
    context: *mut c_void,
) -> i32 {
    let Some(callback) = (unsafe { context.cast::<Callback>().as_ref() }) else {
        return 0;
    };
    // SAFETY: HAL supplies the entire flexible buffer array, not merely the
    // one-element prefix represented in the SDK struct.
    let list = output.as_ptr();
    let count = unsafe { (*list).mNumberBuffers } as usize;
    let pointer = unsafe { std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>() };
    let buffers = unsafe { std::slice::from_raw_parts_mut(pointer, count) };
    unsafe { fill(callback, buffers) };
    0
}

/// This function also runs with owned test buffers; no HAL calls occur here.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
unsafe fn fill(callback: &Callback, buffers: &mut [AudioBuffer]) {
    let _guard = ForbidAlloc::enter();
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
    state.player.render(left, right);
    callback.callbacks.fetch_add(1, Ordering::Relaxed);
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
    use std::time::{Duration, Instant};

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
                        left: vec![0.0; 64],
                        right: vec![0.0; 64],
                    }),
                    failed: AtomicBool::new(false),
                    callbacks: AtomicU64::new(0),
                    channels: 2,
                    silent: false,
                    hardware_epoch: Arc::new(AtomicU64::new(0)),
                    observed_epoch: AtomicU64::new(0),
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
            fill(&a, &mut [buffer(&mut stereo, 2)]);
        }
        unsafe {
            fill(
                &b,
                &mut [
                    buffer(&mut left, 1),
                    buffer(&mut right, 1),
                    buffer(&mut extra, 1),
                ],
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
        unsafe { fill(&a, &mut [buffer(&mut stereo, 2)]) };
        assert!(stereo.iter().all(|sample| *sample == 0.0));
        assert_eq!(control_a.status().sample, held);
        a.observed_epoch.store(1, Ordering::Release);
        unsafe { fill(&a, &mut [buffer(&mut stereo, 2)]) };
        assert_eq!(control_a.status().sample, held + 64);
        assert!(stereo.iter().any(|sample| *sample != 0.0));
        assert!(!a.failed.load(Ordering::Acquire) && !b.failed.load(Ordering::Acquire));
        // A driver enlargement beyond prepared scratch is silent, never a panic.
        let mut large = [1.0f32; 256];
        unsafe {
            fill(&a, &mut [buffer(&mut large, 2)]);
        }
        assert!(large.iter().all(|sample| *sample == 0.0));
        assert!(a.failed.load(Ordering::Acquire));
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
