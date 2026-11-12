//! Shared-memory block protocol between the DAW and a plugin host process.
//!
//! Audio and events live in a locked shared mapping with two block slots.
//! The control socket is separate. The realtime thread never takes a lock.
//! [`MAX_CHANNELS`] and [`MAX_FRAMES`] size the test plugin's activation, not
//! this protocol.

#![cfg_attr(not(target_os = "macos"), allow(unused))]
// Audio crosses this crate on the realtime thread, and the control socket reads
// an untrusted helper's bytes. Product code checks every index, so a bad length
// is an error or a skipped record, not a panic (ADR 0284).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod control;
mod control_decode;
mod control_text;
mod control_transport;
mod descriptors;
mod error;
mod gui;
mod launch_failure;
mod main_reply;
pub mod os_workgroup;
mod params;
mod payload_decode;
mod refusal;
mod refusal_decode;
mod region;
mod session;
mod shortcuts;
mod sync;
mod wire;
mod workgroup;

pub use control::{ControlMessage, Socket};
pub use control_decode::{ControlDecodeError, ControlField};
pub use control_text::ControlTextError;
pub use control_transport::{ControlSendError, DescriptorRuleError};
pub use error::{
    ProtocolError, SessionError, SpawnError, SpawnStep, WorkgroupCall, WorkgroupError,
};
pub use gui::{GuiReport, ParamEvent, ParamEventKind, ParamRelay, PluginActivity};
pub use launch_failure::{LaunchDecodeError, LaunchFailure, LaunchFailureKind, LaunchField};
pub use main_reply::MainReplyKind;
pub use params::{
    PARAM_AUTOMATABLE, PARAM_BYPASS, PARAM_ENUM, PARAM_HIDDEN, PARAM_PERIODIC, PARAM_READONLY,
    PARAM_STEPPED, ParamChange, ParamInfo, ParamText, ParamValue,
};
pub use payload_decode::{ParamField, ParamPayload, PayloadDecodeError, WindowFlag};
pub use refusal::{AudioPort, Pending, PluginFailure, Refusal};
pub use refusal_decode::{RefusalDecodeError, RefusalField};
pub use region::{
    Collected, Delivered, FanIn, Header, MappedRegion, RegionError, SessionLayout,
    SharedMemoryOperation, arm_abort_notice,
};
pub use session::{
    BlockProgress, PluginAudio, PluginControl, PluginHost, PluginNotice, SpawnOptions,
    WorkgroupHandoff,
};
pub use shortcuts::PluginCommand;
pub use sync::{SyncFlags, WaitError, wait_until_ne, wake_one};
pub use workgroup::{
    RetainedWorkgroup, WorkgroupMembership, apply_time_constraint, copy_workgroup_port,
    join_passed_workgroup, join_workgroup,
};

/// Channels the test plugin is activated with. Not a protocol limit.
pub const MAX_CHANNELS: usize = 2;
/// Frames the test plugin is activated with. Not a protocol limit.
pub const MAX_FRAMES: usize = 512;

pub const CMD_PROCESS: u32 = 1;
pub const CMD_SHUTDOWN: u32 = 2;
/// Reset DSP state, then process the block's events and audio in that order.
pub const CMD_RESET_PROCESS: u32 = 3;

/// Adopt prepared storage without changing plugin DSP state.
pub const CMD_ADOPT_MAPPING: u32 = 4;

pub const MAGIC: u64 = 0xEC0D_E001_0000_0004;

/// Sample rate used by the spike host when it activates a plugin.
pub const SPIKE_SAMPLE_RATE: f64 = 48_000.0;
