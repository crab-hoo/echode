//! Loads one CLAP plugin and serves audio blocks from a shared mapping.
//!
//! Without `--plugin`, blocks are copied unchanged. That separates the cost of
//! the crossing from the cost of calling into the plugin.

use clack_host::prelude::*;
use echode_sandbox::{MAX_CHANNELS, MAX_FRAMES, MappedRegion, RegionError, SPIKE_SAMPLE_RATE};
use std::path::Path;

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

struct HostShared;

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

struct EchodeHost;

impl HostHandlers for EchodeHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

#[derive(Debug)]
pub enum HostError {
    Region(RegionError),
    Plugin(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Region(error) => write!(f, "{error}"),
            Self::Plugin(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<RegionError> for HostError {
    fn from(error: RegionError) -> Self {
        Self::Region(error)
    }
}

pub(crate) fn load_entry(path: &Path) -> Result<PluginEntry, HostError> {
    // SAFETY: loading a plugin runs its static constructors in this process.
    // That is the reason this binary is a separate process from the DAW.
    unsafe { PluginEntry::load(path) }.map_err(|error| HostError::Plugin(error.to_string()))
}

pub fn serve(shm: &Path, plugin: Option<&Path>) -> Result<(), HostError> {
    let region = MappedRegion::open(shm)?;
    match plugin {
        None => {
            region.serve(copy_block)?;
            Ok(())
        }
        Some(path) => serve_clap(&region, path),
    }
}

fn copy_block(region: &MappedRegion, frames: usize, channels: usize) -> Result<(), RegionError> {
    for channel in 0..channels {
        // SAFETY: `serve` has already rejected shapes outside the mapping, and
        // this process is the only writer of the output planes during the call.
        unsafe {
            std::ptr::copy_nonoverlapping(
                region.input_ptr(channel),
                region.output_ptr(channel),
                frames,
            );
        }
    }
    Ok(())
}

fn serve_clap(region: &MappedRegion, path: &Path) -> Result<(), HostError> {
    let entry = load_entry(path)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| HostError::Plugin(format!("{} has no plugin factory", path.display())))?;
    let descriptor = factory.plugin_descriptors().next().ok_or_else(|| {
        HostError::Plugin(format!("{} does not contain a plugin", path.display()))
    })?;
    let plugin_id = descriptor
        .id()
        .ok_or_else(|| HostError::Plugin("plugin descriptor has no id".to_string()))?;
    let host_info = HostInfo::new("Echode", "Echode", "https://echode.invalid", "0.0.0")
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let mut instance =
        PluginInstance::<EchodeHost>::new(|_| HostShared, |_| (), &entry, plugin_id, &host_info)
            .map_err(|error| HostError::Plugin(error.to_string()))?;

    let configuration = PluginAudioConfiguration {
        sample_rate: SPIKE_SAMPLE_RATE,
        min_frames_count: 1,
        max_frames_count: MAX_FRAMES as u32,
    };
    let stopped = instance
        .activate(|_, _| (), configuration)
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let mut started = stopped
        .start_processing()
        .map_err(|error| HostError::Plugin(error.to_string()))?;

    let mut input_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut output_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut input = [[0.0f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut output = [[0.0f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut steady = 0u64;

    let served = region.serve(|region, frames, channels| {
        if channels != MAX_CHANNELS {
            return Err(RegionError::BadShape { frames, channels });
        }
        for (channel, plane) in input.iter_mut().enumerate() {
            // SAFETY: the parent published `frames` samples before the request
            // sequence we just observed.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    region.input_ptr(channel),
                    plane.as_mut_ptr(),
                    frames,
                );
            }
        }

        let input_audio = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input
                    .iter_mut()
                    .map(|channel| InputChannel::variable(&mut channel[..frames])),
            ),
        }]);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output.iter_mut().map(|channel| &mut channel[..frames]),
            ),
        }]);
        let input_events = InputEvents::empty();
        let mut output_events = OutputEvents::void();
        started
            .process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                Some(steady),
                None,
            )
            .map_err(|error| RegionError::ProcessFailed(error.to_string()))?;
        steady += frames as u64;

        for (channel, plane) in output.iter().enumerate() {
            // SAFETY: `process` has returned, so the plugin finished writing `output`.
            unsafe {
                std::ptr::copy_nonoverlapping(plane.as_ptr(), region.output_ptr(channel), frames);
            }
        }
        Ok(())
    });

    let stopped = started.stop_processing();
    instance.deactivate(stopped);
    served?;
    Ok(())
}

/// In-process CLAP calls, for comparing against the cross-process round trip.
pub fn measure_in_process(
    plugin: &Path,
    frames: usize,
    iterations: usize,
) -> Result<std::time::Duration, HostError> {
    if frames == 0 || frames > MAX_FRAMES {
        return Err(HostError::Plugin(format!(
            "unsupported frame count {frames}"
        )));
    }
    let entry = load_entry(plugin)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| HostError::Plugin("no factory".to_string()))?;
    let descriptor = factory
        .plugin_descriptors()
        .next()
        .ok_or_else(|| HostError::Plugin("no plugin".to_string()))?;
    let plugin_id = descriptor
        .id()
        .ok_or_else(|| HostError::Plugin("plugin descriptor has no id".to_string()))?;
    let host_info = HostInfo::new("Echode", "Echode", "https://echode.invalid", "0.0.0")
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let mut instance =
        PluginInstance::<EchodeHost>::new(|_| HostShared, |_| (), &entry, plugin_id, &host_info)
            .map_err(|error| HostError::Plugin(error.to_string()))?;
    let stopped = instance
        .activate(
            |_, _| (),
            PluginAudioConfiguration {
                sample_rate: SPIKE_SAMPLE_RATE,
                min_frames_count: 1,
                max_frames_count: MAX_FRAMES as u32,
            },
        )
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let mut started = stopped
        .start_processing()
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let mut input_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut output_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut input = [[0.25f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut output = [[0.0f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut steady = 0u64;
    let started_at = std::time::Instant::now();
    for _ in 0..iterations {
        let input_audio = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input
                    .iter_mut()
                    .map(|channel| InputChannel::variable(&mut channel[..frames])),
            ),
        }]);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output.iter_mut().map(|channel| &mut channel[..frames]),
            ),
        }]);
        let input_events = InputEvents::empty();
        let mut output_events = OutputEvents::void();
        started
            .process(
                &input_audio,
                &mut output_audio,
                &input_events,
                &mut output_events,
                Some(steady),
                None,
            )
            .map_err(|error| HostError::Plugin(error.to_string()))?;
        steady += frames as u64;
    }
    let elapsed = started_at.elapsed();
    let stopped = started.stop_processing();
    instance.deactivate(stopped);
    let _ = output[0][0];
    Ok(elapsed)
}

mod catalog;
mod scan;
mod watch;

pub use catalog::{BundleFound, PluginCatalog, SCAN_QUIET, list_bundles};
pub use scan::{ScannedPlugin, scan_descriptors, scan_report};
pub use watch::FolderWatch;
