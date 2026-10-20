//! Product output. HAL calls and ownership changes happen on the document
//! control thread. Notifications only set an atomic flag.

mod activity;
mod clips;
mod hardware;
mod session;
mod stream;
mod trace;
mod transport;
mod workgroup;

pub use activity::UserActivity;
pub use clips::load_placed_clips;
pub use echode_engine::{ParamChange, ParamInfo, ParamValue, PluginControl, TrackMeter};
pub use hardware::{Inventory, OutputDevice, inventory};
pub use session::{AudioCommand, AudioState, Session};
pub use stream::{
    OutputStream, installed_plugin_routes, project_plugin_ids, routes_with_saved_state,
};
pub use transport::TransportCancellation;

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;
