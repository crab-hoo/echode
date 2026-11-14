//! A lifecycle probe selected by `ECHODE_TEST_FAULT=thread-check`.
//! The snapshot is one byte: successful stages, plus a sticky failure bit.
use clack_plugin::extensions::prelude::*;
use clack_plugin::prelude::PluginError;
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, Ordering};

const INIT: u8 = 1;
const BACKGROUND: u8 = 2;
pub(crate) const ACTIVATE: u8 = 4;
pub(crate) const START: u8 = 8;
pub(crate) const PROCESS: u8 = 16;
pub(crate) const RESET: u8 = 32;
pub(crate) const CALLBACK: u8 = 64;
const FAILED: u8 = 128;
static STAGES: AtomicU8 = AtomicU8::new(0);

pub(crate) fn initialize(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    check(host, INIT)?;
    // Creating and joining a background thread belongs to initialization, not RT.
    let passed = std::thread::scope(|scope| {
        scope
            .spawn(move || check(host, BACKGROUND).is_ok())
            .join()
            .unwrap_or(false)
    });
    if passed {
        Ok(())
    } else {
        Err(PluginError::Message("thread-check background probe failed"))
    }
}

pub(crate) fn check(host: HostSharedHandle<'_>, stage: u8) -> Result<(), PluginError> {
    let Some(extension) = host.get_extension::<HostThreadCheck>() else {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        return Err(PluginError::Message(
            "thread-check extension is unavailable",
        ));
    };
    // SAFETY: this table belongs to the live host. Both CLAP thread checks may
    // be called from any thread, including the audio thread.
    let (main, audio) = unsafe {
        let table = extension.0.as_ptr().as_ref();
        let host = extension.0.host_ptr().as_ptr();
        (
            table.is_main_thread.map(|check| check(host)),
            table.is_audio_thread.map(|check| check(host)),
        )
    };
    let expected_main = matches!(stage, INIT | ACTIVATE | CALLBACK);
    let expected_audio = matches!(stage, START | PROCESS | RESET);
    if main != Some(expected_main) || audio != Some(expected_audio) {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        return Err(PluginError::Message(
            "thread-check reported the wrong thread",
        ));
    }
    STAGES.fetch_or(stage, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn snapshot() -> u8 {
    STAGES.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct HostThreadCheck(RawExtension<HostExtensionSide, clap_host_thread_check>);
// SAFETY: this identifier names the CLAP host thread-check extension.
unsafe impl Extension for HostThreadCheck {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_THREAD_CHECK];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the `clap_host_thread_check` table.
        Self(unsafe { raw.cast() })
    }
}
