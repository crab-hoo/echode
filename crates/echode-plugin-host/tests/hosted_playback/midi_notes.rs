use super::*;

/// The engine's project notes reach a MIDI-only note port as MIDI 1.0 at the
/// same samples as native CLAP notes, at every rate and block partition.
#[test]
fn midi_only_note_ports_match_native_note_onsets_under_rtsan() {
    let native = compare_note_onsets(&plugin_dylib(), "dev.echode.test-instrument", Some("onset"));
    // The fixture writes 1.0 from any native note-on or note-off it receives,
    // so a host that ignored the port's dialect cannot match the native voice.
    let midi = compare_note_onsets(
        &plugin_dylib(),
        "dev.echode.test-instrument",
        Some("onset-midi"),
    );
    assert_eq!(native.len(), 2);
    assert!(
        midi == native,
        "MIDI-only notes differ from native CLAP notes"
    );
}
