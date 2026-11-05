use super::*;

const RATE: u32 = 48_000;

#[test]
fn the_extracted_dsp_matches_the_pinned_wavesynth_samples_exactly() {
    for velocity in [0.0, 1.0 / 65535.0, 0.137, 0.5, 1.0] {
        let mut synth = SinePad::new(RATE).unwrap();
        synth.prepare(2, RATE).unwrap();
        // This block and two voices bound the reference fixture, not polyphony.
        let mut actual = [0.0; 128];
        let events = [
            Event::note_on(0, 0, 1, 60, velocity),
            Event::note_on(17, 0, 2, 67, 1.0),
            Event::note_off(41, 0, 1, 60),
        ];
        synth.process(&mut [&mut actual], &events, RATE).unwrap();
        let mut first = Voice::new(RATE as f32, Instrument::SinePad);
        let mut second = Voice::new(RATE as f32, Instrument::SinePad);
        let stored = (velocity * 65535.0).round() as u16;
        first.note_on(60, (stored >> 9) as u8);
        for (frame, actual) in actual.into_iter().enumerate() {
            if frame == 17 {
                second.note_on(67, 127);
            }
            if frame == 41 {
                first.note_off();
            }
            let mut expected = 0.0;
            if first.is_active() {
                expected += first.next_sample();
            }
            if second.is_active() {
                expected += second.next_sample();
            }
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "frame {frame}, velocity {velocity}"
            );
        }
    }
}

#[test]
fn legacy_state_bytes_tail_and_latency_are_unchanged() {
    let mut synth = SinePad::new(RATE).unwrap();
    let mut bytes = Vec::new();
    synth.save(&mut bytes);
    assert_eq!(bytes, b"\x01SinePad");
    synth.restore(b"\x01SinePad").unwrap();
    for invalid in [b"".as_slice(), b"\x02SinePad", b"\x01SinePad\x00"] {
        assert_eq!(synth.restore(invalid), Err(DspError::InvalidState));
    }
    assert_eq!(synth.latency_samples(), 0);
    assert_eq!(synth.tail_samples(RATE), 19_200);
}

#[test]
fn prepared_voice_transfer_preserves_held_and_releasing_samples() {
    for held in [true, false] {
        let mut old = SinePad::new(RATE).unwrap();
        let mut reference = SinePad::new(RATE).unwrap();
        old.prepare(1, RATE).unwrap();
        reference.prepare(1, RATE).unwrap();
        let mut scratch = [0.0; 23]; // This fixture deliberately splits the attack.
        let on = [Event::note_on(0, 0, 1, 69, 1.0)];
        old.process(&mut [&mut scratch], &on, RATE).unwrap();
        scratch.fill(0.0);
        reference.process(&mut [&mut scratch], &on, RATE).unwrap();
        let mut next = SinePad::new(RATE).unwrap();
        next.prepare(2, RATE).unwrap();
        {
            let _guard = ForbidAlloc::enter();
            next.carry_from(&mut old, |_| Some(held));
        }
        let off = [Event::note_off(0, 0, 1, 69)];
        let reference_events = if held { &[][..] } else { &off[..] };
        let mut actual = [0.0; 64];
        let mut expected = [0.0; 64];
        next.process(&mut [&mut actual], &[], RATE).unwrap();
        reference
            .process(&mut [&mut expected], reference_events, RATE)
            .unwrap();
        assert_eq!(actual, expected);
        assert!(old.voices.is_empty());
        next.silence();
        actual.fill(0.0);
        next.process(&mut [&mut actual], &[], RATE).unwrap();
        assert_eq!(actual, [0.0; 64]);
    }
}

#[test]
fn invalid_rate_events_and_unprepared_capacity_are_rejected() {
    assert!(matches!(SinePad::new(0), Err(DspError::InvalidSampleRate)));
    let mut synth = SinePad::new(RATE).unwrap();
    let mut output = [0.0; 8]; // Only the invalid-event fixture's buffer length.
    let on = Event::note_on(0, 0, 1, 60, 1.0);
    assert_eq!(
        synth.process(&mut [&mut output], &[on], RATE),
        Err(DspError::BufferTooSmall)
    );
    synth.prepare(2, RATE).unwrap();
    assert_eq!(
        synth.process(&mut [&mut output], &[], 44_100),
        Err(DspError::InvalidSampleRate)
    );
    let late = Event::note_on(8, 0, 1, 60, 1.0);
    assert_eq!(
        synth.process(&mut [&mut output], &[late], RATE),
        Err(DspError::BufferTooSmall)
    );
    let reversed = [
        Event::note_on(2, 0, 1, 60, 1.0),
        Event::note_on(1, 0, 2, 60, 1.0),
    ];
    assert_eq!(
        synth.process(&mut [&mut output], &reversed, RATE),
        Err(DspError::Time)
    );
}
