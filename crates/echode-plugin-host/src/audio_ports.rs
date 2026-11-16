//! Audio-port rescans. A plugin may change any part of its ports while inactive.
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT, CLAP_AUDIO_PORTS_RESCAN_FLAGS,
    CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR, CLAP_AUDIO_PORTS_RESCAN_LIST,
    CLAP_AUDIO_PORTS_RESCAN_NAMES, CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE, CLAP_EXT_AUDIO_PORTS,
    clap_audio_port_info, clap_host_audio_ports,
};
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use echode_sandbox::PluginFailure;
use std::ffi::CStr;
use std::ptr;

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

/// Read `port_count` audio-port channel counts through a CLAP `get` callback.
pub(crate) fn read_port_channels(
    port_count: u32,
    mut get: impl FnMut(u32, *mut clap_audio_port_info) -> bool,
    mut on_undescribed: impl FnMut(u32) -> PluginFailure,
    mut on_zero_channels: impl FnMut(u32) -> PluginFailure,
) -> Result<Vec<u32>, PluginFailure> {
    let mut counts = Vec::with_capacity(usize::try_from(port_count).map_or(64, |n| n.min(64)));
    for index in 0..port_count {
        let mut info = clap_audio_port_info {
            id: 0,
            name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
            flags: 0,
            channel_count: 0,
            port_type: ptr::null(),
            in_place_pair: CLAP_INVALID_ID,
        };
        if !get(index, &mut info) {
            return Err(on_undescribed(index));
        }
        if info.channel_count == 0 {
            return Err(on_zero_channels(index));
        }
        counts.push(info.channel_count);
    }
    Ok(counts)
}
