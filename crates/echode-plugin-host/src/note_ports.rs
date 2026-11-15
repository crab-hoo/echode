//! Advertise native CLAP notes and MIDI 1.0 for ports without the CLAP dialect.
use crate::threads::EchodeHost;
use clack_host::events::event_types::MidiEvent;
use clack_host::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI,
    CLAP_NOTE_DIALECT_MIDI_MPE, clap_host_note_ports,
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
    // Project notes are native CLAP. A port without the CLAP dialect that
    // accepts MIDI 1.0 receives them as MIDI channel messages (`events`).
    // MPE and MIDI 2.0 have no translation, so they are not claimed.
    CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI
}

unsafe extern "C" fn rescan(_host: *const clap_host, _flags: u32) {
    // Names and output descriptors are not retained. The derived input state
    // (`NoteInputs`) is rebuilt immediately before EVERY activation, after
    // init/deactivate has returned. ALL is only legal while inactive, so there
    // is no active snapshot to invalidate or reentrant scan to perform here.
    // Active structural changes require request_restart first.
}

/// What the input note ports need, read while the plugin is inactive and
/// moved with its processor. Main builds and drops it; audio only reads it.
pub(crate) struct NoteInputs {
    /// MIDI all-sounds-off for every channel of every MIDI-capable port.
    pub(crate) resets: Vec<MidiEvent>,
    /// By input port index: project notes travel as MIDI 1.0.
    pub(crate) midi_ports: Vec<bool>,
}

impl NoteInputs {
    pub(crate) const fn none() -> Self {
        Self {
            resets: Vec::new(),
            midi_ports: Vec::new(),
        }
    }

    /// Adds the next input port with the dialects its descriptor declares.
    ///
    /// Events address ports by `u16`, so later ports never receive project
    /// notes. A MIDI-capable port there still needs its resets: an error.
    pub(crate) fn add_port(&mut self, index: u32, supported_dialects: u32) -> Result<(), ()> {
        let midi = supported_dialects & (CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE) != 0;
        let Ok(port) = u16::try_from(index) else {
            return if midi { Err(()) } else { Ok(()) };
        };
        debug_assert_eq!(
            self.midi_ports.len(),
            usize::from(port),
            "ports in index order"
        );
        // Native CLAP keeps note ids and expressions. Use MIDI only without it.
        self.midi_ports.push(
            supported_dialects & CLAP_NOTE_DIALECT_CLAP == 0
                && supported_dialects & CLAP_NOTE_DIALECT_MIDI != 0,
        );
        // All-sounds-off is a valid MIDI 1.0 message under MPE as well.
        if midi {
            // MIDI 1.0 defines sixteen channels per port. Port count is dynamic.
            for channel in 0..16 {
                self.resets
                    .push(MidiEvent::new(0, port, [0xB0 | channel, 120, 0]));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::NoteInputs;
    use clap_sys::ext::note_ports::{
        CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE,
        CLAP_NOTE_DIALECT_MIDI2,
    };

    #[test]
    fn only_ports_without_clap_notes_receive_midi_notes() {
        let mut inputs = NoteInputs::none();
        for (port, dialects) in [
            CLAP_NOTE_DIALECT_CLAP,
            CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI,
            CLAP_NOTE_DIALECT_MIDI,
            CLAP_NOTE_DIALECT_MIDI_MPE,
            CLAP_NOTE_DIALECT_MIDI2,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            inputs.add_port(port as u32, dialects).unwrap();
        }
        // Ports past u16 cannot receive notes; only MIDI resets need an index.
        assert_eq!(inputs.add_port(1 << 16, CLAP_NOTE_DIALECT_CLAP), Ok(()));
        assert_eq!(inputs.add_port(1 << 16, CLAP_NOTE_DIALECT_MIDI), Err(()));
        assert_eq!(inputs.midi_ports, [false, false, true, false, false, false]);
        let reset_ports: Vec<_> = inputs
            .resets
            .iter()
            .map(|reset| reset.port_index())
            .collect();
        let expected: Vec<_> = [1u16, 2, 3]
            .into_iter()
            .flat_map(|port| [port; 16])
            .collect();
        assert_eq!(reset_ports, expected);
    }
}
