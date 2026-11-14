//! Advertise native note events without promising an unimplemented MIDI route.
use crate::threads::EchodeHost;
use clack_host::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, clap_host_note_ports,
};
use clap_sys::host::clap_host;
use std::ffi::CStr;

#[derive(Clone, Copy)]
pub(crate) struct HostNotePorts;

// SAFETY: this identifier selects CLAP's official host note-ports table.
unsafe impl Extension for HostNotePorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_NOTE_PORTS];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: the implementation has the official clap_host_note_ports C layout.
// Both callbacks are stateless and never access the host pointer.
unsafe impl ExtensionImplementation<EchodeHost> for HostNotePorts {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_note_ports {
            supported_dialects: Some(supported_dialects),
            rescan: Some(rescan),
        });
}

unsafe extern "C" fn supported_dialects(_host: *const clap_host) -> u32 {
    // Project notes and expression are native CLAP. MIDI all-sounds-off is a
    // lifecycle fallback, not support for project MIDI, MPE, or MIDI 2.0.
    CLAP_NOTE_DIALECT_CLAP
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names and output descriptors are not retained. The sole derived input
    // state (MIDI lifecycle resets) is rebuilt immediately before EVERY
    // activation, after init/deactivate has returned. ALL is only legal while
    // inactive, so there is no active snapshot to invalidate or reentrant scan
    // to perform here. Active structural changes require request_restart first.
}
