use super::{NoteDialect, NoteInputs};
use clack_host::events::Event;
use clap_sys::ext::note_ports::{
    CLAP_NOTE_DIALECT_CLAP as CLAP, CLAP_NOTE_DIALECT_MIDI as MIDI,
    CLAP_NOTE_DIALECT_MIDI_MPE as MPE, CLAP_NOTE_DIALECT_MIDI2 as MIDI2,
};

#[test]
fn every_dialect_combination_preserves_existing_routes_and_resets() {
    let mut inputs = NoteInputs::none();
    for flags in 0..16 {
        inputs.add_port(flags, flags).unwrap();
        let expected = if flags & CLAP != 0 {
            NoteDialect::Clap
        } else if flags & MIDI != 0 {
            NoteDialect::Midi
        } else if flags & MIDI2 != 0 {
            NoteDialect::Midi2
        } else if flags & MPE != 0 {
            NoteDialect::Mpe
        } else {
            NoteDialect::Clap
        };
        assert_eq!(inputs.dialects[flags as usize], expected, "flags={flags}");
    }
    // Only the MPE-only port gets a zone (`mpe_tests.rs` checks its configuration).
    assert_eq!(
        inputs
            .zones
            .iter()
            .map(|zone| zone.port())
            .collect::<Vec<_>>(),
        [MPE as u16]
    );
    let midi: Vec<_> = (0..16u16)
        .filter(|flags| u32::from(*flags) & (MIDI | MPE) != 0)
        .flat_map(|port| (0..16).map(move |channel| (port, [0xB0 | channel, 120, 0])))
        .collect();
    assert_eq!(
        inputs
            .resets
            .iter()
            .map(|event| (event.port_index(), event.data()))
            .collect::<Vec<_>>(),
        midi
    );
    let midi2: Vec<_> = (0..16u16)
        .filter(|flags| u32::from(*flags) & MIDI2 != 0)
        .flat_map(|port| (0..16).map(move |channel| (port, [0x40B0_7800 | channel << 16, 0, 0, 0])))
        .collect();
    assert_eq!(
        inputs
            .midi2_resets
            .iter()
            .map(|event| (event.port_index(), event.data()))
            .collect::<Vec<_>>(),
        midi2
    );
    assert!(inputs.resets.iter().all(|event| event.header().time() == 0));
    assert!(
        inputs
            .midi2_resets
            .iter()
            .all(|event| event.header().time() == 0)
    );
}

#[test]
fn unaddressable_midi_ports_are_refused_including_midi2_only_ports() {
    let mut inputs = NoteInputs::none();
    for flags in [CLAP, 0] {
        assert_eq!(inputs.add_port(1 << 16, flags), Ok(()));
    }
    for flags in [MIDI, MPE, MIDI2, CLAP | MIDI2] {
        assert_eq!(inputs.add_port(1 << 16, flags), Err(()));
    }
    for port in 0..=u16::MAX {
        inputs
            .add_port(u32::from(port), if port == u16::MAX { MIDI2 } else { CLAP })
            .unwrap();
    }
    assert_eq!(inputs.dialects.last(), Some(&NoteDialect::Midi2));
    assert_eq!(inputs.midi2_resets.len(), 16);
    assert!(
        inputs
            .midi2_resets
            .iter()
            .all(|event| event.port_index() == u16::MAX)
    );
}

#[test]
fn a_reset_or_new_run_forgets_members_and_configures_once_per_block() {
    let mut inputs = NoteInputs::none();
    inputs.add_port(0, MPE).unwrap();
    // A fresh activation configures its first block, then not again.
    assert!(inputs.take_configure());
    assert!(!inputs.take_configure());
    assert_eq!(inputs.zones[0].start(1, 60), 1);
    assert_eq!(inputs.zones[0].start(2, 60), 2);
    let _guard = crate::realtime::ForbidAlloc::enter();
    inputs.start_over();
    assert!(inputs.take_configure());
    assert_eq!(inputs.zones[0].end(1, 60), None);
    assert_eq!(inputs.zones[0].start(3, 60), 1);
}
