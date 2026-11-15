//! Dimension-preserving audio-port metadata can change while inactive.
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORTS_RESCAN_FLAGS, CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR,
    CLAP_AUDIO_PORTS_RESCAN_NAMES, CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE, CLAP_EXT_AUDIO_PORTS,
    clap_host_audio_ports,
};
use clap_sys::host::clap_host;
use std::ffi::CStr;

#[derive(Clone, Copy)]
pub(crate) struct HostAudioPorts;

// SAFETY: this identifier names CLAP's host audio-ports extension.
unsafe impl Extension for HostAudioPorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: the implementation has the official clap_host_audio_ports C layout.
// Neither callback dereferences the host or mutates processing state.
unsafe impl ExtensionImplementation<EchodeHost> for HostAudioPorts {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_audio_ports {
            is_rescan_flag_supported: Some(is_rescan_flag_supported),
            rescan: Some(rescan),
        });
}

unsafe extern "C" fn is_rescan_flag_supported(_host: *const clap_host, flag: u32) -> bool {
    // CLAP asks about one flag at a time, not a set of flags. Precision hints,
    // semantic port types, and optional aliasing do not change our separate f32
    // buffers or index-based routing. Dimensions and port lists remain fixed.
    matches!(
        flag,
        CLAP_AUDIO_PORTS_RESCAN_NAMES
            | CLAP_AUDIO_PORTS_RESCAN_FLAGS
            | CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE
            | CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR
    )
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names and these metadata hints are not cached. All audio uses separate
    // f32 planes regardless of precision preferences, port type, or alias hints.
    // Before reactivation the main thread revalidates the immutable port layout;
    // do not reenter a plugin's descriptor callbacks from its rescan callback.
}
