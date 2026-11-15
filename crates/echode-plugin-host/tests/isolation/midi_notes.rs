use super::*;

const NATIVE_ON: u8 = 1;
const NATIVE_OFF: u8 = 2;
const MIDI: u8 = 4;

/// Kind, port, sample offset and the complete wire payload.
type Seen = (u8, u16, u32, [u32; 4]);

fn midi(status: u8, key: u8, value: u8) -> [u32; 4] {
    [
        u32::from(status) | u32::from(key) << 8 | u32::from(value) << 16,
        0,
        0,
        0,
    ]
}

/// Host dialects, activations, the fixture's port layout, and the latest block.
fn snapshot(host: &PluginHost) -> ([u8; 3], Vec<Seen>) {
    let state = host
        .control_handle()
        .capture_state()
        .expect("midi-notes snapshot");
    assert_eq!(state.len(), 4 + 64 * 24, "{state:?}");
    assert_ne!(state[3], u8::MAX, "the block overflowed the fixture's log");
    let seen = state[4..]
        .chunks_exact(24)
        .take(usize::from(state[3]))
        .map(|chunk| {
            let address = u64::from_le_bytes(chunk[..8].try_into().unwrap());
            let data = std::array::from_fn(|index| {
                u32::from_le_bytes(chunk[8 + index * 4..12 + index * 4].try_into().unwrap())
            });
            (
                address as u8,
                (address >> 8) as u16,
                (address >> 24) as u32,
                data,
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
    // The host claims CLAP, MIDI 1.0 and MIDI 2.0, not MPE.
    // Port 1 starts MIDI 1.0-only and port 0 CLAP-only.
    wait_for(&host, [0b1011, 1, 0b10]);
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
            (NATIVE_ON, 0, 2, [60, 0, 0, 0]),
            (MIDI, 1, 3, midi(0x92, 61, 100)),
            (MIDI, 1, 5, midi(0x82, 61, 0)),
            (NATIVE_OFF, 0, 7, [60, 0, 0, 0]),
        ],
    );
    // CLAP permits note-port changes only while inactive. The routes must be
    // read again before reactivation, not kept from the first activation.
    host.control_handle()
        .restore_state(&[0b01])
        .expect("swap the port dialects");
    wait_for(&host, [0b1011, 2, 0b01]);
    check(
        &mut host,
        &[
            (MIDI, 0, 2, midi(0x90, 60, 127)),
            (NATIVE_ON, 1, 3, [61, 0, 0, 0]),
            (NATIVE_OFF, 1, 5, [61, 0, 0, 0]),
            (MIDI, 0, 7, midi(0x80, 60, 0)),
        ],
    );
}

#[test]
fn midi2_ports_refresh_notes_and_all_sounds_off_on_every_activation_under_rtsan() {
    let mut host = spawn_plugin(Some("midi-notes"), SessionLayout::stereo());
    wait_for(&host, [0b1011, 1, 0b10]);
    // Change MIDI 1.0 -> MIDI 2.0, swap native/UMP ports, remove UMP, restore.
    for (step, layout) in [0x82, 0x81, 0, 0x82].into_iter().enumerate() {
        host.control_handle().restore_state(&[layout]).unwrap();
        wait_for(&host, [0b1011, step as u8 + 2, layout]);
        for frames in [8, 256] {
            for reset in [false, true] {
                let port = if layout == 0x81 { 0 } else { 1 };
                let events = [
                    note(true, 0, port, 15, 127, 0.0),
                    note(true, 2, 1 - port, 0, 60, 1.0),
                    note(false, 5, port, 15, 127, 32768.0 / 65535.0),
                ];
                if reset {
                    host.request_reset();
                }
                let input = stereo(frames, 0.0);
                let mut output = stereo(frames, 0.0);
                host.process_block(
                    frames,
                    &[&input[0], &input[1]],
                    &mut outs(&mut output),
                    &events,
                    &mut [],
                    Duration::from_secs(2),
                )
                .unwrap();
                let mut expected = Vec::new();
                if reset {
                    expected.push((3, u16::MAX, 0, [0xFF, 0, 0, 0]));
                    if layout != 0 {
                        for channel in 0..16 {
                            expected.push((7, port, 0, [0x40B0_7800 | channel << 16, 0, 0, 0]));
                        }
                    }
                }
                if layout != 0 {
                    expected.push((7, port, 0, [0x409F_7F00, 0, 0, 0]));
                    expected.push((NATIVE_ON, 1 - port, 2, [60, 0, 0, 0]));
                    expected.push((7, port, 5, [0x408F_7F00, 0x8000_0000, 0, 0]));
                } else {
                    expected.extend([
                        (NATIVE_ON, port, 0, [127, 0, 0, 0]),
                        (NATIVE_ON, 1 - port, 2, [60, 0, 0, 0]),
                        (NATIVE_OFF, port, 5, [127, 0, 0, 0]),
                    ]);
                }
                assert_eq!(
                    snapshot(&host).1,
                    expected,
                    "MIDI2 note/reset stream: layout={layout}, frames={frames}, reset={reset}"
                );
            }
        }
    }
}
