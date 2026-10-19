use super::*;

fn note(tag_on: bool) -> Event {
    let note = NoteEvent {
        time: 3,
        track: 4,
        target: Target::note(9, 60),
        velocity: 0.5,
    };
    if tag_on {
        Event::NoteOn(note)
    } else {
        Event::NoteOff(note)
    }
}

#[test]
fn every_event_round_trips() {
    let events = [
        Event::Transport(Transport {
            time: 1,
            playing: true,
            recording: false,
            loop_active: true,
            tempo_bpm: 120.0,
            tempo_inc: 0.0,
            song_pos_beats: 1.5,
            song_pos_seconds: 0.75,
            loop_start_beats: 0.0,
            loop_end_beats: 4.0,
            loop_start_seconds: 0.0,
            loop_end_seconds: 2.0,
            bar_start: 0.0,
            bar_number: -2,
            signature_numerator: 4,
            signature_denominator: 4,
        }),
        note(true),
        note(false),
        Event::NoteChoke(ChokeEvent {
            time: 8,
            track: 1,
            target: Target::any(),
        }),
        Event::Expression(ExpressionEvent {
            time: 2,
            track: 1,
            target: Target::note(3, 61),
            kind: ExpressionKind::Pressure,
            value: 0.25,
        }),
        Event::ParamValue(ParamEvent {
            time: 0,
            track: 2,
            param_id: 7,
            target: Target::any(),
            value: 0.125,
        }),
        Event::ParamMod(ParamEvent {
            time: 5,
            track: 2,
            param_id: 8,
            target: Target::any(),
            value: -0.5,
        }),
    ];
    for event in events {
        let record = encode_event(&event).expect("encode");
        assert_eq!(decode_event(&record), Some(event));
    }
}

#[test]
fn invalid_records_are_none() {
    assert!(decode_event(&[0; 16]).is_none());
    let mut record = encode_event(&note(true)).expect("encode");
    record[OFF_TAG] = 99;
    assert!(decode_event(&record).is_none());
    let mut transport = encode_event(&Event::Transport(Transport {
        time: 0,
        playing: true,
        recording: false,
        loop_active: false,
        tempo_bpm: 100.0,
        tempo_inc: 0.0,
        song_pos_beats: 0.0,
        song_pos_seconds: 0.0,
        loop_start_beats: 0.0,
        loop_end_beats: 0.0,
        loop_start_seconds: 0.0,
        loop_end_seconds: 0.0,
        bar_start: 0.0,
        bar_number: 0,
        signature_numerator: 4,
        signature_denominator: 4,
    }))
    .expect("encode");
    transport[OFF_FLAGS] = 2;
    assert!(decode_event(&transport).is_none());
    let mut expression = encode_event(&Event::Expression(ExpressionEvent {
        time: 0,
        track: 0,
        target: Target::any(),
        kind: ExpressionKind::Pitch,
        value: 0.0,
    }))
    .expect("encode");
    expression[OFF_FLAGS] = 9;
    assert!(decode_event(&expression).is_none());
    assert!(
        encode_event(&Event::NoteOn(NoteEvent {
            time: 0,
            track: 0,
            target: Target {
                port: None,
                channel: None,
                key: None,
                note_id: Some(u32::MAX),
            },
            velocity: 1.0,
        }))
        .is_none()
    );
}
