//! Shared-memory block protocol between the DAW and a plugin host process.
//!
//! The audio bytes live in a file-backed mapping. Each block is published with
//! a sequence counter and woken with `os_sync_wait_on_address`, so the realtime
//! thread never takes a lock. This spike is stereo and at most [`MAX_FRAMES`]
//! frames per block; the production protocol will not keep that ceiling.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod region;
mod session;
mod sync;
mod workgroup;

pub use region::{Header, MappedRegion, Region, RegionError};
pub use session::{PluginHost, SessionError, SpawnOptions, WorkgroupHandoff};
pub use sync::{SyncFlags, WaitError};
pub use workgroup::{
    WorkgroupMembership, copy_workgroup_port, join_passed_workgroup, join_workgroup,
};

/// Stereo only, for this spike. Production routing is not capped at two channels.
pub const MAX_CHANNELS: usize = 2;
/// Largest block this spike mapping can hold. Not a limit of the DAW.
pub const MAX_FRAMES: usize = 512;

pub const CMD_PROCESS: u32 = 1;
pub const CMD_SHUTDOWN: u32 = 2;

pub const MAGIC: u64 = 0xEC0D_E001_0000_0001;

/// Sample rate used by the spike host when it activates a plugin.
pub const SPIKE_SAMPLE_RATE: f64 = 48_000.0;
