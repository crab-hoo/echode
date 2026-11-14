//! A changing declared tail, selected by the fixture's saved byte.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_host_tail, clap_plugin_tail};
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, Ordering};

static SAMPLES: AtomicU32 = AtomicU32::new(0);

pub(crate) fn report(samples: u32) {
    SAMPLES.store(samples, Ordering::Release);
}

pub(crate) fn initialize(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    host.get_extension::<HostTail>()
        .ok_or(PluginError::Message("tail extension is unavailable"))?;
    Ok(())
}

pub(crate) fn process(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    let samples = match crate::STATE_BYTE.load(Ordering::Acquire) {
        0 => 0,
        1 => u32::MAX,
        _ => 1024,
    };
    let previous = SAMPLES.swap(samples, Ordering::AcqRel);
    if previous != samples {
        let extension = host
            .get_extension::<HostTail>()
            .ok_or(PluginError::Message("tail extension is unavailable"))?;
        // SAFETY: the extension and host belong to this live instance; CLAP
        // allows the changed notice on the audio thread running this process.
        unsafe {
            if let Some(changed) = extension.0.as_ptr().as_ref().changed {
                changed(extension.0.host_ptr().as_ptr());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct PluginTail;
// SAFETY: this identifier names the CLAP plugin tail extension.
unsafe impl Extension for PluginTail {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_TAIL];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation has the official clap_plugin_tail C layout.
unsafe impl ExtensionImplementation<crate::TestGain> for PluginTail {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_tail { get: Some(get) });
}
unsafe extern "C" fn get(_plugin: *const clap_plugin) -> u32 {
    SAMPLES.load(Ordering::Acquire)
}

#[derive(Clone, Copy)]
struct HostTail(RawExtension<HostExtensionSide, clap_host_tail>);
// SAFETY: this identifier names the CLAP host tail extension.
unsafe impl Extension for HostTail {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_TAIL];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier above selects a clap_host_tail table.
        Self(unsafe { raw.cast() })
    }
}
