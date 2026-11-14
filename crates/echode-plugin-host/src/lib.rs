//! Loads one CLAP plugin and serves audio blocks from a shared mapping.
//!
//! Without `--plugin`, blocks are copied unchanged. That separates the cost of
//! the crossing from the cost of calling into the plugin.

// This process runs the plugin's audio thread. Product code checks every
// index, so a bad count is an error or a skipped event, not a panic that
// stops the helper (ADR 0284).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use clack_host::prelude::*;
use echode_sandbox::{
    MAX_CHANNELS, MAX_FRAMES, PluginFailure, RegionError, SPIKE_SAMPLE_RATE, WorkgroupError,
};
use std::os::fd::FromRawFd;
use std::path::Path;

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod awake;
mod gui;
mod launch;
pub use launch::LoadError;
mod main_requests;
mod main_signal;
mod relay;
pub use awake::HelperActivity;

/// Why the helper could not load, start or serve its plugin (ADR 0295).
#[derive(Debug)]
pub enum HostError {
    Region(RegionError),
    Load(LoadError),
    /// Calling the instance during a measurement failed.
    Instance(PluginInstanceError),
    /// The plugin lacks what the helper needs, or answered `false`.
    Plugin(PluginFailure),
    StartRefused,
    ProcessRefused,
    /// The control socket failed.
    Socket(std::io::Error),
    Workgroup(WorkgroupError),
    RunLoopSource,
    NoRunLoop,
    /// The processing thread looked for the plugin's ports before the main
    /// thread stored them. The launch order rules this out.
    PortsUnpublished,
    /// The named worker thread panicked. It has already printed why.
    Panicked(&'static str),
    /// A measurement asked for a block size the buffers cannot hold.
    FrameCount(usize),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Region(error) => write!(f, "{error}"),
            Self::Load(error) => write!(f, "{error}"),
            Self::Instance(error) => write!(f, "{error}"),
            Self::Plugin(failure) => write!(f, "{failure}"),
            Self::StartRefused => f.write_str("plugin refused to start processing"),
            Self::ProcessRefused => f.write_str("process refused"),
            Self::Socket(error) => write!(f, "{error}"),
            Self::Workgroup(error) => write!(f, "{error}"),
            Self::RunLoopSource => f.write_str("could not create the main run loop source"),
            Self::NoRunLoop => f.write_str("the thread has no run loop"),
            Self::PortsUnpublished => f.write_str("plugin ports were not published"),
            Self::Panicked(thread) => write!(f, "the {thread} thread panicked"),
            Self::FrameCount(frames) => write!(f, "unsupported frame count {frames}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(error) => Some(error),
            Self::Region(error) => Some(error),
            Self::Instance(error) => Some(error),
            Self::Plugin(error) => Some(error),
            Self::Socket(error) => Some(error),
            Self::Workgroup(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LoadError> for HostError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

impl From<RegionError> for HostError {
    fn from(error: RegionError) -> Self {
        Self::Region(error)
    }
}

impl From<PluginFailure> for HostError {
    fn from(failure: PluginFailure) -> Self {
        Self::Plugin(failure)
    }
}

/// The identity the helper gives every plugin. Fixed C strings, so it cannot fail.
pub(crate) fn host_info() -> HostInfo {
    HostInfo::new_from_cstring(
        c"Echode".into(),
        c"Echode".into(),
        c"https://echode.invalid".into(),
        c"0.0.0".into(),
    )
}

pub(crate) fn load_entry(path: &Path) -> Result<PluginEntry, LoadError> {
    // SAFETY: loading a plugin runs its static constructors in this process.
    // That is the reason this binary is a separate process from the DAW.
    unsafe { PluginEntry::load(path) }.map_err(LoadError::Entry)
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
    // SAFETY: the parent placed the control socket on `control_fd` before exec, and nothing else in
    // this process owns it.
    let control = unsafe { std::os::fd::OwnedFd::from_raw_fd(control_fd) };
    // SAFETY: the parent placed the fan-in word on this descriptor before exec, and nothing else in
    // this process owns it.
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
        return Err(HostError::FrameCount(frames));
    }
    let entry = load_entry(plugin)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| LoadError::NoFactory(plugin.to_path_buf()))?;
    let descriptor = factory
        .plugin_descriptors()
        .next()
        .ok_or_else(|| LoadError::NoPlugin(plugin.to_path_buf()))?;
    let plugin_id = descriptor.id().ok_or(LoadError::NoPluginId)?;
    let mut instance = PluginInstance::<threads::EchodeHost>::new(
        |_| threads::HostShared::inert(),
        threads::host_main,
        &entry,
        plugin_id,
        &host_info(),
    )
    .map_err(HostError::Instance)?;
    let stopped = instance
        .activate(
            |_, _| (),
            PluginAudioConfiguration {
                sample_rate: SPIKE_SAMPLE_RATE,
                min_frames_count: 1,
                max_frames_count: MAX_FRAMES as u32,
            },
        )
        .map_err(HostError::Instance)?;
    let audio_thread = threads::AudioFlag::enter();
    let mut started = stopped
        .start_processing()
        .map_err(|_| HostError::StartRefused)?;
    let mut input_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut output_ports = AudioPorts::with_capacity(MAX_CHANNELS, 1);
    let mut input = [[0.25f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut output = [[0.0f32; MAX_FRAMES]; MAX_CHANNELS];
    let mut steady = 0u64;
    let started_at = std::time::Instant::now();
    for _ in 0..iterations {
        let input_audio = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(input.iter_mut().map(|channel| {
                InputChannel::variable(channel.get_mut(..frames).unwrap_or_default())
            })),
        }]);
        let mut output_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                output
                    .iter_mut()
                    .map(|channel| channel.get_mut(..frames).unwrap_or_default()),
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
            .map_err(HostError::Instance)?;
        steady += frames as u64;
    }
    let elapsed = started_at.elapsed();
    let stopped = started.stop_processing();
    drop(audio_thread);
    instance.deactivate(stopped);
    let _ = output[0][0];
    Ok(elapsed)
}

mod catalog;
mod events;
mod extension;
mod live;
mod realtime;
mod scan;
mod threads;
mod watch;

pub use catalog::{
    BundleFound, BundleRevision, PluginCatalog, SCAN_QUIET, bundle_binary, catalog_bundles,
    catalog_roots, clap_path_directories, list_bundles, list_bundles_including_clap_path,
};
pub use events::write_input;
pub use live::{CatalogTiming, CatalogWaiter, LiveCatalog};
#[cfg(debug_assertions)]
pub use realtime::GuardAlloc;
pub use scan::{
    CatalogError, ChildState, ResolvedPlugin, SCAN_BUDGET, SLOW_SCAN_BUDGET, ScanCacheEntry,
    ScanError, ScanFailure, ScanProgress, ScanUnread, ScannedPlugin, cache_entry, cached_plugins,
    exit_with_parent, host_binary, installed_clap_bundles, load_scan_cache, refresh_catalog,
    refresh_catalog_within, resolve_cached, resolve_installed, scan_descriptors,
    scan_out_of_process, scan_path, scan_report, scan_with_budget, shared_cache_path,
    store_scan_cache,
};
pub use watch::{FolderWatch, WatchError};
