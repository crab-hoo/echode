//! Product output. HAL calls and ownership changes happen on the document
//! control thread. Notifications only set an atomic flag.

mod activity;
mod audition;
mod clips;
mod hardware;
mod io_cycle;
mod opening;
mod panic_log;
mod session;
mod stream;
mod trace;
mod transport;
mod workgroup;

pub use activity::UserActivity;
pub use audition::{
    Audition, AuditionEntry, AuditionInput, AuditionOrigin, AuditionPhase, AuditionReport,
    AuditionSettings, AuditionWaveform, sample_overview,
};
pub use clips::load_placed_clips;
pub use echode_engine::{
    GuiReport, ParamChange, ParamEvent, ParamEventKind, ParamInfo, ParamRelay, ParamText,
    ParamValue, PluginActivity, PluginCommand, PluginControl, TrackMeter,
};
pub use hardware::{Inventory, OutputDevice, inventory};
#[cfg(any(test, feature = "headless-output"))]
pub use opening::HeadlessOutput;
pub use panic_log::keep_panic_log;
pub use session::{AudioCommand, AudioState, DeviceAccess, MasterAutoMute, Session, TrackAutoMute};
pub use stream::{
    OutputStream, installed_plugin_routes, project_plugin_ids, routes_with_saved_state,
};
pub use transport::TransportCancellation;

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;
