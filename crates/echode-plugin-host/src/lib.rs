//! Loads one CLAP plugin and serves audio blocks from a shared mapping.
//!
//! Without `--plugin`, blocks are copied unchanged. That separates the cost of
//! the crossing from the cost of calling into the plugin.

use clack_host::prelude::*;
use echode_sandbox::{MAX_CHANNELS, MAX_FRAMES, RegionError, SPIKE_SAMPLE_RATE};
use std::os::fd::FromRawFd;
use std::path::Path;

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

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

pub fn serve(
    shm: &Path,
    plugin: Option<&Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    threads::serve(shm, plugin, plugin_id, sample_rate, workgroup)
}

pub fn serve_fds(
    shm_fd: i32,
    control_fd: i32,
    fan_fd: Option<i32>,
    plugin: Option<&Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    // SAFETY: the parent placed the shared mapping on `shm_fd`, the control
    // socket on `control_fd`, and the optional fan-in word on `fan_fd`, before
    // exec. This process owns them.
    let shm = unsafe { std::os::fd::OwnedFd::from_raw_fd(shm_fd) };
    let control = unsafe { std::os::fd::OwnedFd::from_raw_fd(control_fd) };
    let fan = fan_fd.map(|fd| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
    threads::serve_fds(shm, control, fan, plugin, plugin_id, sample_rate, workgroup)
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
    let mut instance = PluginInstance::<threads::EchodeHost>::new(
        |_| threads::HostShared::inert(),
        |_| (),
        &entry,
        plugin_id,
        &host_info,
    )
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
mod events;
mod live;
mod scan;
mod threads;
mod watch;

pub use catalog::{
    BundleFound, BundleRevision, PluginCatalog, SCAN_QUIET, bundle_binary, catalog_bundles,
    catalog_roots, clap_path_directories, list_bundles, list_bundles_including_clap_path,
};
pub use events::write_input;
pub use live::{CatalogWaiter, LiveCatalog};
pub use scan::{
    ResolvedPlugin, SCAN_BUDGET, ScanCacheEntry, ScannedPlugin, cache_entry, cached_plugins,
    host_binary, installed_clap_bundles, load_scan_cache, refresh_catalog, resolve_installed,
    scan_descriptors, scan_out_of_process, scan_path, scan_report, shared_cache_path,
    store_scan_cache,
};
pub use watch::FolderWatch;
