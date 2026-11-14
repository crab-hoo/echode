//! Probes the host's conservative audio-port rescan capabilities on main.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORTS_RESCAN_NAMES, CLAP_EXT_AUDIO_PORTS, clap_host_audio_ports,
};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, Ordering};

const FAILED: u8 = 0x80;
static STAGES: AtomicU8 = AtomicU8::new(0);
static ACTIVATIONS: AtomicU8 = AtomicU8::new(0);

pub(crate) fn probe(host: HostSharedHandle<'_>, stage: u8) {
    let Some(extension) = host.get_extension::<HostAudioPorts>() else {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        return;
    };
    // SAFETY: the live host owns this table. Both calls run during initialization
    // or on_main_thread, as required by CLAP's audio-ports extension.
    unsafe {
        let table = extension.0.as_ptr().as_ref();
        let host = extension.0.host_ptr().as_ptr();
        let (Some(supported), Some(rescan)) = (table.is_rescan_flag_supported, table.rescan) else {
            STAGES.fetch_or(FAILED, Ordering::Relaxed);
            return;
        };
        // Check every single bit, including future/unknown flags, and invalid
        // combinations. The product must not advertise mutable buffer layouts.
        let passed = (0..u32::BITS).all(|bit| {
            let flag = 1u32 << bit;
            supported(host, flag) == (flag == CLAP_AUDIO_PORTS_RESCAN_NAMES)
        }) && !supported(host, 0)
            && !supported(host, u32::MAX)
            && !supported(host, CLAP_AUDIO_PORTS_RESCAN_NAMES | (1 << 2));
        STAGES.fetch_or(if passed { stage } else { FAILED }, Ordering::Relaxed);
        rescan(host, CLAP_AUDIO_PORTS_RESCAN_NAMES);
    }
}

pub(crate) fn activated() {
    ACTIVATIONS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn snapshot() -> [u8; 2] {
    [
        STAGES.load(Ordering::Relaxed),
        ACTIVATIONS.load(Ordering::Relaxed),
    ]
}

pub(crate) fn name() -> [std::ffi::c_char; clap_sys::string_sizes::CLAP_NAME_SIZE] {
    let mut name = [0; clap_sys::string_sizes::CLAP_NAME_SIZE];
    name[0] = if STAGES.load(Ordering::Relaxed) & 2 == 0 {
        b'A' as std::ffi::c_char
    } else {
        b'B' as std::ffi::c_char
    };
    name
}

#[derive(Clone, Copy)]
struct HostAudioPorts(RawExtension<HostExtensionSide, clap_host_audio_ports>);
// SAFETY: this identifier selects the official CLAP host audio-ports table.
unsafe impl Extension for HostAudioPorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the clap_host_audio_ports C layout.
        Self(unsafe { raw.cast() })
    }
}
