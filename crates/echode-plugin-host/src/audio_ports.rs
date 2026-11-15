//! Audio-port rescans. A plugin may change any part of its ports while inactive.
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT, CLAP_AUDIO_PORTS_RESCAN_FLAGS,
    CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR, CLAP_AUDIO_PORTS_RESCAN_LIST,
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
    // buffers or index-based routing. Channel counts and port lists are read
    // again before each activation and reach the parent's routing (ADR 0382).
    matches!(
        flag,
        CLAP_AUDIO_PORTS_RESCAN_NAMES
            | CLAP_AUDIO_PORTS_RESCAN_FLAGS
            | CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT
            | CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE
            | CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR
            | CLAP_AUDIO_PORTS_RESCAN_LIST
    )
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names and metadata hints are not cached. Every activation reads the
    // ports after deactivate returns, and the processor carries what it read,
    // so no rescan needs an action here. Do not reenter a plugin's descriptor
    // callbacks from its rescan callback.
}
