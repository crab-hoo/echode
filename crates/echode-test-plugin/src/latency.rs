//! Latency metadata: synthetic for `latency`, the real length for `delay`,
//! a number without a delay for `claim`.
//! One helper owns one instance.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::plugin::clap_plugin;
use std::cell::Cell;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, Ordering};

static SAMPLES: AtomicU32 = AtomicU32::new(0);
thread_local! {
    static MAIN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn activated(host: &HostMainThreadHandle<'_>) {
    MAIN.with(|main| main.set(true));
    SAMPLES.fetch_add(256, Ordering::Relaxed);
    if SAMPLES.load(Ordering::Relaxed) == 768 {
        SAMPLES.store(0, Ordering::Relaxed);
    }
    if let Some(extension) = host.get_extension::<HostLatency>() {
        // SAFETY: the extension and host belong to this live instance. Activation
        // runs on the main thread, where CLAP permits the latency change notice.
        unsafe {
            if let Some(changed) = extension.0.as_ptr().as_ref().changed {
                changed(extension.0.host_ptr().as_ptr());
            }
        }
    }
}

static CLAIMED: AtomicU32 = AtomicU32::new(0);

/// Reads the `<frames>` of `claim:<frames>`. A malformed number claims none.
pub(crate) fn claim(frames: &str) {
    CLAIMED.store(frames.parse().unwrap_or(0), Ordering::Relaxed);
}

pub(crate) fn claimed() -> u32 {
    CLAIMED.load(Ordering::Relaxed)
}

/// A fixed latency for this activation, read on the activating thread.
pub(crate) fn report(samples: u32) {
    MAIN.with(|main| main.set(true));
    SAMPLES.store(samples, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
pub(crate) struct PluginLatency;
// SAFETY: `CLAP_EXT_LATENCY` is the official identifier of the extension this type stands for.
unsafe impl Extension for PluginLatency {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_LATENCY];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_latency`, the `#[repr(C)]` table CLAP declares
// for that identifier.
unsafe impl ExtensionImplementation<crate::TestGain> for PluginLatency {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_latency { get: Some(get) });
}
unsafe extern "C" fn get(_plugin: *const clap_plugin) -> u32 {
    assert!(
        MAIN.with(Cell::get),
        "latency must be read on the activation thread"
    );
    SAMPLES.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct HostLatency(RawExtension<HostExtensionSide, clap_host_latency>);
// SAFETY: `CLAP_EXT_LATENCY` is the official identifier of the extension this type stands for.
unsafe impl Extension for HostLatency {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_LATENCY];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: `IDENTIFIERS` names the latency extension, whose host table is
        // `clap_host_latency`, so the raw extension clack found has this type.
        Self(unsafe { raw.cast() })
    }
}
