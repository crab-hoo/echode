//! Advertise the note dialects the helper can translate before each activation.
use crate::mpe::MpeZone;
use crate::threads::EchodeHost;
use clack_host::events::event_types::{Midi2Event, MidiEvent};
use clack_host::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI,
    CLAP_NOTE_DIALECT_MIDI_MPE, CLAP_NOTE_DIALECT_MIDI2, clap_host_note_ports,
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
    // MPE-only ports get a configured Lower Zone and member channels (`mpe.rs`).
    CLAP_NOTE_DIALECT_CLAP
        | CLAP_NOTE_DIALECT_MIDI
        | CLAP_NOTE_DIALECT_MIDI_MPE
        | CLAP_NOTE_DIALECT_MIDI2
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names and output descriptors are not retained. The derived input state
    // (`NoteInputs`) is rebuilt immediately before EVERY activation, after
    // init/deactivate has returned. ALL is only legal while inactive, so there
    // is no active snapshot to invalidate or reentrant scan to perform here.
    // Active structural changes require request_restart first.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteDialect {
    Clap,
    Midi,
    Midi2,
    /// MIDI 1.0 on member channels of a fifteen-channel Lower Zone.
    Mpe,
}

/// What the input note ports need, read while the plugin is inactive and
/// moved with its processor. Main builds and drops it. Audio reads it and only
/// counts or clears the MPE zones' fixed storage; it never grows or frees it.
pub(crate) struct NoteInputs {
    /// MIDI all-sounds-off for every channel of every MIDI-capable port.
    pub(crate) resets: Vec<MidiEvent>,
    /// MIDI 2.0 all-sounds-off, group zero (the group used for project notes).
    pub(crate) midi2_resets: Vec<Midi2Event>,
    pub(crate) dialects: Vec<NoteDialect>,
    /// Member-channel assignment of every MPE port, in port order.
    pub(crate) zones: Vec<MpeZone>,
    /// The next block starts a processing run or follows a reset, so it
    /// configures the zones before any note.
    configure: bool,
}

impl NoteInputs {
    pub(crate) const fn none() -> Self {
        Self {
            resets: Vec::new(),
            midi2_resets: Vec::new(),
            dialects: Vec::new(),
            zones: Vec::new(),
            configure: true,
        }
    }

    /// No voice sounds: a processing run starts or the plugin was reset.
    /// Forget member assignments and configure the zones again first.
    pub(crate) fn start_over(&mut self) {
        self.zones.iter_mut().for_each(MpeZone::clear);
        self.configure = true;
    }

    /// Whether this block must send each zone's configuration, after the
    /// resets. Asked once per block.
    pub(crate) fn take_configure(&mut self) -> bool {
        std::mem::take(&mut self.configure)
    }

    /// Adds the next input port with the dialects its descriptor declares.
    ///
    /// Events address ports by `u16`, so later ports never receive project
    /// notes. A MIDI-capable port there still needs its resets: an error.
    pub(crate) fn add_port(&mut self, index: u32, supported_dialects: u32) -> Result<(), ()> {
        let midi = supported_dialects & (CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE) != 0;
        let midi2 = supported_dialects & CLAP_NOTE_DIALECT_MIDI2 != 0;
        let Ok(port) = u16::try_from(index) else {
            return if midi || midi2 { Err(()) } else { Ok(()) };
        };
        debug_assert_eq!(
            self.dialects.len(),
            usize::from(port),
            "ports in index order"
        );
        // Keep existing native, MIDI 1.0 and MIDI 2.0 routes unchanged. Only
        // use MPE when it is the one dialect the port takes.
        let dialect = if supported_dialects & CLAP_NOTE_DIALECT_CLAP != 0 {
            NoteDialect::Clap
        } else if supported_dialects & CLAP_NOTE_DIALECT_MIDI != 0 {
            NoteDialect::Midi
        } else if midi2 {
            NoteDialect::Midi2
        } else if midi {
            NoteDialect::Mpe
        } else {
            NoteDialect::Clap
        };
        self.dialects.push(dialect);
        if dialect == NoteDialect::Mpe {
            self.zones.push(MpeZone::new(port));
        }
        // MIDI defines sixteen channels per group/port. Port count is dynamic.
        for channel in 0..16 {
            // All-sounds-off is a valid MIDI 1.0 message under MPE as well.
            if midi {
                self.resets
                    .push(MidiEvent::new(0, port, [0xB0 | channel, 120, 0]));
            }
            if midi2 {
                self.midi2_resets.push(Midi2Event::new(
                    0,
                    port,
                    [0x40B0_7800 | u32::from(channel) << 16, 0, 0, 0],
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "note_ports_tests.rs"]
mod tests;
