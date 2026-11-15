use super::*;

const NATIVE_ON: u8 = 1;
const NATIVE_OFF: u8 = 2;
const MIDI: u8 = 4;

/// One input event the fixture saw: kind, port, time, then the MIDI bytes or
/// the native key in the first byte.
type Seen = (u8, u8, u16, [u8; 3]);

/// Host dialects, activations, the fixture's port layout, and the latest block.
fn snapshot(host: &PluginHost) -> ([u8; 3], Vec<Seen>) {
    let state = host
        .control_handle()
        .capture_state()
        .expect("midi-notes snapshot");
    assert_eq!(state.len(), 4 + 16 * 8, "{state:?}");
    assert_ne!(state[3], u8::MAX, "the block overflowed the fixture's log");
    let seen = state[4..]
        .chunks_exact(8)
        .take(usize::from(state[3]))
        .map(|chunk| {
            let record = u64::from_le_bytes(chunk.try_into().unwrap());
            let data = (record >> 32).to_le_bytes();
            (
                record as u8,
                (record >> 8) as u8,
                (record >> 16) as u16,
                [data[0], data[1], data[2]],
            )
        })
        .collect();
    ([state[0], state[1], state[2]], seen)
}

fn wait_for(host: &PluginHost, expected: [u8; 3]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (state, _) = snapshot(host);
        if state == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "midi-notes fixture did not reach {expected:?}: {state:?}; {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn note(on: bool, time: u32, port: u16, channel: u16, key: u8, velocity: f64) -> Event {
    let note = NoteEvent {
        time,
        track: 1,
        target: Target {
            port: Some(port),
            channel: Some(channel),
            key: Some(key),
            note_id: Some(5),
        },
        velocity,
    };
    if on {
        Event::NoteOn(note)
    } else {
        Event::NoteOff(note)
    }
}

#[test]
fn midi_only_note_ports_receive_midi_1_notes_and_follow_inactive_changes_under_rtsan() {
    let layout = SessionLayout {
        input_events: 16,
        ..SessionLayout::stereo()
    };
    let mut host = spawn_plugin(Some("midi-notes"), layout);
    // The host claims CLAP and MIDI 1.0 (bits 0 and 1), not MPE or MIDI 2.0.
    // Port 1 starts MIDI-only and port 0 CLAP-only.
    wait_for(&host, [0b11, 1, 0b10]);
    let events = [
        note(true, 2, 0, 0, 60, 1.0),
        note(true, 3, 1, 2, 61, 100.0 / 127.0),
        note(false, 5, 1, 2, 61, 0.0),
        note(false, 7, 0, 0, 60, 0.0),
    ];
    let check = |host: &mut PluginHost, expected: &[Seen]| {
        // Eight frames hold this fixture's latest event; 256 is a common block.
        for frames in [8, 256] {
            let input = stereo(frames, 0.0);
            let mut output = stereo(frames, 1.0);
            host.process_block(
                frames,
                &[&input[0], &input[1]],
                &mut outs(&mut output),
                &events,
                &mut [],
                Duration::from_secs(2),
            )
            .expect("notes reach both dialects");
            assert_eq!(snapshot(host).1, expected, "{frames} frames");
        }
    };
    check(
        &mut host,
        &[
            (NATIVE_ON, 0, 2, [60, 0, 0]),
            (MIDI, 1, 3, [0x92, 61, 100]),
            (MIDI, 1, 5, [0x82, 61, 0]),
            (NATIVE_OFF, 0, 7, [60, 0, 0]),
        ],
    );
    // CLAP permits note-port changes only while inactive. The routes must be
    // read again before reactivation, not kept from the first activation.
    host.control_handle()
        .restore_state(&[0b01])
        .expect("swap the port dialects");
    wait_for(&host, [0b11, 2, 0b01]);
    check(
        &mut host,
        &[
            (MIDI, 0, 2, [0x90, 60, 127]),
            (NATIVE_ON, 1, 3, [61, 0, 0]),
            (NATIVE_OFF, 1, 5, [61, 0, 0]),
            (MIDI, 0, 7, [0x80, 60, 0]),
        ],
    );
}
