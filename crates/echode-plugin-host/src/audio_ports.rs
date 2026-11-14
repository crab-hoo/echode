//! Audio-port names may change without invalidating prepared buffer layouts.
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORTS_RESCAN_NAMES, CLAP_EXT_AUDIO_PORTS, clap_host_audio_ports,
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
    // The prepared graph fixes port/channel counts and buffer policy. Do not
    // promise structural changes, unknown flags, or combinations of flags.
    flag == CLAP_AUDIO_PORTS_RESCAN_NAMES
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names are not retained or displayed, so there is no name cache to refresh.
    // In particular, a name-only change must not restart or interrupt audio.
    // Plugins must query each supported flag before requesting a rescan; flags
    // other than NAMES are not supported and cannot alter this host's layout.
}
