//! A plugin's CLAP extension tables, looked up and cast in one place (ADR 0321).

use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;

/// The plugin's table for extension `id`, or `None` when it has none.
///
/// # Safety
/// `T` is the `#[repr(C)]` table the CLAP headers declare for `id`.
pub(crate) unsafe fn table<'a, T>(plugin: &'a clap_plugin, id: &CStr) -> Option<&'a T> {
    let get = plugin.get_extension?;
    // SAFETY: `get_extension` is the plugin's own function, which CLAP lets any
    // thread call with that plugin and a NUL-terminated identifier.
    let table = unsafe { get(plugin, id.as_ptr()) };
    // SAFETY: a non-null result is the plugin's table for `id`, a `T` by the
    // caller's contract. CLAP keeps it valid while the plugin lives, which `'a`
    // borrows.
    unsafe { table.cast::<T>().as_ref() }
}
