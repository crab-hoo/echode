//! Product output. HAL calls and ownership changes happen on the document
//! control thread. Notifications only set an atomic flag.

mod hardware;
mod session;
mod stream;

pub use hardware::{Inventory, OutputDevice, inventory};
pub use session::{AudioCommand, AudioState, Session};
pub use stream::OutputStream;

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;
