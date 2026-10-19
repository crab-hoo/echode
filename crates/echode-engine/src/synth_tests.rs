use super::*;
fn on(offset: u32, note: u32, pitch: u8) -> Event {
    Event::note_on(offset, 0, note, pitch, 1.0)
}

fn off(offset: u32, note: u32) -> Event {
    Event::note_off(offset, 0, note, 60)
}

fn slots<const N: usize>(rate: u32) -> [SynthVoice; N] {
    std::array::from_fn(|_| SynthVoice::new(rate).expect("sample rate"))
}

#[test]
fn a_sine_pad_starts_silent_and_then_sounds() {
    // Four slots are this test's scratch, not a polyphony cap.
    let mut voices = slots::<4>(48_000);
    let mut output = [0.0; 64];
    render_test_synth(&[on(0, 2, 69)], &mut voices, 48_000, &mut output).unwrap();
    assert_eq!(output[0], 0.0);
    assert!(output[32].is_finite() && output[32] != 0.0);
    assert!(voices.iter().any(|voice| voice.inner.is_active()));
}

#[test]
fn split_blocks_match_one_render_and_a_second_pass_matches() {
    let events = [on(0, 2, 60), on(0, 3, 64)];
    let mut together = [0.0; 32];
    let mut voices = slots::<4>(48_000);
    render_test_synth(&events, &mut voices, 48_000, &mut together).unwrap();

    let mut head = [0.0; 16];
    let mut tail = [0.0; 16];
    let mut voices = slots::<4>(48_000);
    render_test_synth(&events, &mut voices, 48_000, &mut head).unwrap();
    render_test_synth(&[], &mut voices, 48_000, &mut tail).unwrap();
    assert_eq!(together[..16], head);
    assert_eq!(together[16..], tail);

    let mut again = [0.0; 32];
    let mut voices = slots::<4>(48_000);
    render_test_synth(&events, &mut voices, 48_000, &mut again).unwrap();
    assert_eq!(again, together);

    let mut left = [0.0; 32];
    let mut right = [0.0; 32];
    render_test_synth(&[on(0, 2, 60)], &mut slots::<4>(48_000), 48_000, &mut left).unwrap();
    render_test_synth(&[on(0, 3, 64)], &mut slots::<4>(48_000), 48_000, &mut right).unwrap();
    for index in 0..32 {
        assert_eq!(together[index], left[index] + right[index]);
    }
}

#[test]
fn the_synth_rejects_a_bad_rate_a_short_buffer_and_a_late_event() {
    let mut voices = slots::<1>(48_000);
    let mut output = [0.0; 4];
    assert_eq!(
        render_test_synth(&[on(0, 2, 60)], &mut voices, 0, &mut output).unwrap_err(),
        PlaybackError::InvalidSampleRate
    );
    assert_eq!(
        render_test_synth(
            &[on(0, 2, 60)],
            &mut slots::<1>(44_100),
            48_000,
            &mut output
        )
        .unwrap_err(),
        PlaybackError::InvalidSampleRate
    );
    assert_eq!(
        render_test_synth(
            &[on(0, 2, 60), on(0, 3, 62)],
            &mut voices,
            48_000,
            &mut output
        )
        .unwrap_err(),
        PlaybackError::BufferTooSmall
    );
    assert_eq!(
        render_test_synth(&[on(4, 2, 60)], &mut voices, 48_000, &mut output).unwrap_err(),
        PlaybackError::BufferTooSmall
    );
    assert_eq!(
        render_test_synth(
            &[on(2, 2, 60), on(1, 3, 62)],
            &mut voices,
            48_000,
            &mut output
        )
        .unwrap_err(),
        PlaybackError::Time
    );
}

#[test]
fn one_second_of_one_note_stays_finite() {
    let mut voices = slots::<4>(48_000);
    let mut output = [0.0; 64];
    for block in 0..(48_000 / 64) {
        output.fill(0.0);
        if block == 0 {
            render_test_synth(&[on(0, 2, 69)], &mut voices, 48_000, &mut output).unwrap();
        } else {
            render_test_synth(&[], &mut voices, 48_000, &mut output).unwrap();
        }
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
}

#[test]
fn note_off_keeps_the_voice_until_the_release_ends() {
    let mut voices = slots::<1>(48_000);
    let mut output = [0.0; 8];
    render_test_synth(&[on(0, 2, 60), off(1, 2)], &mut voices, 48_000, &mut output).unwrap();
    assert!(voices[0].inner.is_active());
    assert_eq!(
        render_test_synth(&[on(0, 3, 64)], &mut voices, 48_000, &mut output).unwrap_err(),
        PlaybackError::BufferTooSmall
    );
}

#[test]
fn the_test_synth_runs_under_rtsan() {
    let mut voices = slots::<1>(48_000);
    let mut output = [0.0; 64];
    let event = on(0, 1, 69);
    render_one_block(&mut voices, &mut output, &event);
    assert!(output.iter().any(|sample| *sample != 0.0));
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn render_one_block(voices: &mut [SynthVoice], output: &mut [f32], event: &Event) {
    render_test_synth(std::slice::from_ref(event), voices, 48_000, output).unwrap();
}

fn sampler_slots<const N: usize>() -> [SamplerVoice; N] {
    std::array::from_fn(|_| SamplerVoice::idle())
}

#[test]
fn the_sampler_copies_the_root_pitch_and_skips_an_octave_up() {
    let pcm = [0.25, 0.5, -0.75, 1.0];
    let mut root = [0.0; 6];
    render_test_sampler(
        &[on(0, 2, 60)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut sampler_slots::<2>(),
        &mut root,
    )
    .unwrap();
    assert_eq!(root, [0.25, 0.5, -0.75, 1.0, 0.0, 0.0]);

    let mut octave = [0.0; 4];
    render_test_sampler(
        &[on(0, 2, 72)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut sampler_slots::<2>(),
        &mut octave,
    )
    .unwrap();
    assert_eq!(octave, [0.25, -0.75, 0.0, 0.0]);

    let mut held = [0.0; 4];
    render_test_sampler(
        &[on(0, 2, 60)],
        &pcm,
        60,
        SamplerInterpolation::Floor,
        &mut sampler_slots::<2>(),
        &mut held,
    )
    .unwrap();
    assert_eq!(held, [0.25, 0.5, -0.75, 1.0]);
}

#[test]
fn sampler_blocks_stay_aligned_with_one_pass() {
    let pcm = [0.2, -0.4, 0.6, -0.8, 0.1];
    let mut together = [0.0; 8];
    render_test_sampler(
        &[on(1, 4, 60)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut sampler_slots::<2>(),
        &mut together,
    )
    .unwrap();

    let mut head = [0.0; 4];
    let mut tail = [0.0; 4];
    let mut voices = sampler_slots::<2>();
    render_test_sampler(
        &[on(1, 4, 60)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut voices,
        &mut head,
    )
    .unwrap();
    render_test_sampler(
        &[],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut voices,
        &mut tail,
    )
    .unwrap();
    assert_eq!(together[..4], head);
    assert_eq!(together[4..], tail);
    assert_eq!(together[0], 0.0);
    assert_eq!(together[1], 0.2);
}

#[test]
fn sinc_playback_matches_across_passes() {
    let pcm = [0.2, -0.4, 0.6, -0.8, 0.1, 0.3, -0.2, 0.5];
    let mut once = [0.0; 16];
    render_test_sampler(
        &[on(0, 2, 60)],
        &pcm,
        60,
        SamplerInterpolation::Sinc,
        &mut sampler_slots::<1>(),
        &mut once,
    )
    .unwrap();
    let mut again = [0.0; 16];
    render_test_sampler(
        &[on(0, 2, 60)],
        &pcm,
        60,
        SamplerInterpolation::Sinc,
        &mut sampler_slots::<1>(),
        &mut again,
    )
    .unwrap();
    assert_eq!(once, again);
    assert!(once.iter().any(|sample| *sample != 0.0));
    assert!(once.iter().all(|sample| sample.is_finite()));
}

#[test]
fn a_semitone_up_makes_floor_and_linear_differ() {
    let pcm = [0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, -1.0];
    let mut floor = [0.0; 32];
    let mut linear = [0.0; 32];
    render_test_sampler(
        &[on(0, 2, 61)],
        &pcm,
        60,
        SamplerInterpolation::Floor,
        &mut sampler_slots::<1>(),
        &mut floor,
    )
    .unwrap();
    render_test_sampler(
        &[on(0, 2, 61)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut sampler_slots::<1>(),
        &mut linear,
    )
    .unwrap();
    assert_ne!(floor, linear);
}

#[test]
fn the_synth_device_fills_every_plane_and_remembers_its_instrument() {
    use crate::Device;
    let mut device = TestSynth::new(48_000).unwrap();
    device.prepare(1, 48_000).unwrap();
    assert_eq!(device.latency_samples(), 0);
    assert_eq!(device.tail_samples(48_000), 19_200);
    let mut left = [0.0; 64];
    let mut right = [0.0; 64];
    let mut planes: [&mut [f32]; 2] = [&mut left, &mut right];
    device
        .process(&mut planes, &[on(0, 1, 69)], 48_000)
        .unwrap();
    assert_eq!(left, right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    let mut saved = Vec::new();
    device.save(&mut saved);
    device.restore(&saved).unwrap();
    assert!(device.restore(&[]).is_err());
    let through: &mut dyn Device = &mut device;
    assert_eq!(through.tail_samples(48_000), 19_200);
}

#[test]
fn the_sampler_device_switches_interpolation_and_restores_it() {
    use crate::Device;
    use echode_realtime::{ParamEvent, Target};
    let pcm = [0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, -1.0];
    let mut linear = TestSampler::new(&pcm, 60, 48_000).unwrap();
    linear.prepare(1, 48_000).unwrap();
    assert_eq!(linear.latency_samples(), 0);
    assert_eq!(linear.tail_samples(48_000), 0);
    let mut saved = Vec::new();
    linear.save(&mut saved);
    let mut output = [0.0; 32];
    let mut planes = [&mut output[..]];
    linear
        .process(
            &mut planes,
            &[
                on(0, 2, 61),
                Event::ParamValue(ParamEvent {
                    time: 0,
                    track: 0,
                    param_id: SAMPLER_INTERPOLATION,
                    target: Target::any(),
                    value: 0.0,
                }),
            ],
            48_000,
        )
        .unwrap();
    let mut direct = [0.0; 32];
    render_test_sampler(
        &[on(0, 2, 61)],
        &pcm,
        60,
        SamplerInterpolation::Floor,
        &mut sampler_slots::<1>(),
        &mut direct,
    )
    .unwrap();
    assert_eq!(output, direct);
    linear.restore(&saved).unwrap();
    let mut again = [0.0; 32];
    let mut planes = [&mut again[..]];
    linear
        .process(&mut planes, &[on(0, 2, 61)], 48_000)
        .unwrap();
    let mut linear_direct = [0.0; 32];
    render_test_sampler(
        &[on(0, 2, 61)],
        &pcm,
        60,
        SamplerInterpolation::Linear,
        &mut sampler_slots::<1>(),
        &mut linear_direct,
    )
    .unwrap();
    assert_eq!(again, linear_direct);
    assert!(linear.restore(&[1, 9]).is_err());
}
