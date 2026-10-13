//! Product output. HAL calls and ownership changes happen on the document
//! control thread. Notifications only set an atomic flag.

mod clips;
mod hardware;
mod session;
mod stream;

pub use clips::load_placed_clips;
pub use hardware::{Inventory, OutputDevice, inventory};
pub use session::{AudioCommand, AudioState, Session};
pub use stream::{OutputStream, installed_plugin_routes, project_plugin_ids};

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;
