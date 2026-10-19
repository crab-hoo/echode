//! Optional CLAP extension for hosts that prepare an entire score's event budget.
//! The host calls `prepare` on the plugin main thread before publishing a larger
//! event mapping. The plugin must prepare storage without touching its live DSP.
//! The count bounds distinct note identities as well as events in one block.

use std::ffi::{CStr, c_void};

pub const PLUGIN_EVENT_CAPACITY: &CStr = c"dev.echode.event-capacity/1";

#[repr(C)]
pub struct PluginEventCapacity {
    pub prepare: unsafe extern "C" fn(plugin: *const c_void, events: u32) -> bool,
}
