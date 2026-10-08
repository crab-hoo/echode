//! Built-in test synth and sampler.
//!
//! The synth is wavesynth's Sine Pad. The sampler plays caller PCM through
//! dasp's floor, linear, or sinc interpolator. Voice slots belong to the
//! caller: the slice length is scratch for that render, not a polyphony cap.
//! Rendering adds into the caller buffer and does not allocate. The same
//! events and stored voices always write the same samples.

use dasp_interpolate::Interpolator;
use dasp_interpolate::floor::Floor;
use dasp_interpolate::linear::Linear;
use dasp_interpolate::sinc::Sinc;
use dasp_ring_buffer::Fixed;
use echode_model::Id;
use wavesynth::engine::voice::Voice;
use wavesynth::instrument::Instrument;

use crate::realtime::ForbidAlloc;
use crate::sequence::{MidiEvent, MidiKind};
use crate::transport::PlaybackError;

/// wavesynth crate version used for the test synth.
pub const TEST_SYNTH_ID: &str = "wavesynth";
/// Pinned crate version.
pub const TEST_SYNTH_VERSION: &str = "0.1.0";
/// Preset rendered by [`render_test_synth`].
pub const TEST_SYNTH_INSTRUMENT: &str = "SinePad";
/// dasp interpolators used by the test sampler.
pub const TEST_SAMPLER_ID: &str = "dasp_interpolate";
/// Pinned dasp_interpolate version.
pub const TEST_SAMPLER_VERSION: &str = "0.11.0";

/// How a test sampler reads between PCM samples.
///
/// All three are dasp 0.11. Linear is the default because a whole-sample step
/// returns the stored sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SamplerInterpolation {
    Floor,
    #[default]
    Linear,
    Sinc,
}

/// Ring length for dasp's sinc. Depth is half of this. The window belongs to
/// the interpolator; it is not a limit on how long a recording may be.
const SINC_FRAMES: usize = 16;

/// One wavesynth voice, built for a single sample rate.
pub struct SynthVoice {
    inner: Voice,
    sample_rate: u32,
    note: Option<Id>,
}

impl SynthVoice {
    /// Builds a silent Sine Pad voice. `sample_rate` must be above zero.
    pub fn new(sample_rate: u32) -> Result<Self, PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        Ok(Self {
            inner: Voice::new(sample_rate as f32, Instrument::SinePad),
            sample_rate,
            note: None,
        })
    }
}

/// One sampler voice. The sinc window is only used for [`SamplerInterpolation::Sinc`].
pub struct SamplerVoice {
    note: Option<Id>,
    position: u64,
    step: u64,
    gain: f32,
    active: bool,
    fed: u64,
    sinc: Sinc<[f32; SINC_FRAMES]>,
}

impl SamplerVoice {
    pub fn idle() -> Self {
        Self {
            note: None,
            position: 0,
            step: 0,
            gain: 0.0,
            active: false,
            fed: 0,
            sinc: fresh_sinc(),
        }
    }
}

fn fresh_sinc() -> Sinc<[f32; SINC_FRAMES]> {
    Sinc::new(Fixed::from([0.0; SINC_FRAMES]))
}

/// Adds a Sine Pad into `output`.
///
/// Events must be ordered by `sample_offset`. A note-on with velocity 0 is a
/// note-off. Note-off starts the preset's release instead of cutting the
/// sample. Every voice must have been built at `sample_rate`. The buffer is
/// unspecified when this returns an error.
pub fn render_test_synth(
    events: &[MidiEvent],
    voices: &mut [SynthVoice],
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    if sample_rate == 0 || voices.iter().any(|voice| voice.sample_rate != sample_rate) {
        return Err(PlaybackError::InvalidSampleRate);
    }
    let _guard = ForbidAlloc::enter();
    drive(events, output, voices, note_on_synth, mix_synth)
}

/// Adds a one-shot sampler into `output`.
///
/// `pcm` is already at the device rate. `root` is the pitch that plays it at
/// its stored speed. Playback stops at the end of `pcm` and on note-off.
pub fn render_test_sampler(
    events: &[MidiEvent],
    pcm: &[f32],
    root: u8,
    interpolation: SamplerInterpolation,
    voices: &mut [SamplerVoice],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    let _guard = ForbidAlloc::enter();
    drive(
        events,
        output,
        voices,
        |voices, event| note_on_sampler(voices, event, root),
        |voices| mix_sampler(voices, pcm, interpolation),
    )
}

fn drive<T: ?Sized>(
    events: &[MidiEvent],
    output: &mut [f32],
    state: &mut T,
    mut handle: impl FnMut(&mut T, &MidiEvent) -> Result<(), PlaybackError>,
    mut mix: impl FnMut(&mut T) -> f32,
) -> Result<(), PlaybackError> {
    check_events(events, output.len())?;
    let mut index = 0;
    for (frame, sample) in output.iter_mut().enumerate() {
        let at = u32::try_from(frame).map_err(|_| PlaybackError::Time)?;
        while index < events.len() && events[index].sample_offset == at {
            handle(state, &events[index])?;
            index += 1;
        }
        *sample += mix(state);
    }
    Ok(())
}

fn check_events(events: &[MidiEvent], frames: usize) -> Result<(), PlaybackError> {
    let mut previous = None;
    for event in events {
        let offset = usize::try_from(event.sample_offset).map_err(|_| PlaybackError::Time)?;
        if offset >= frames {
            return Err(PlaybackError::BufferTooSmall);
        }
        if let Some(previous) = previous
            && offset < previous
        {
            return Err(PlaybackError::Time);
        }
        previous = Some(offset);
    }
    Ok(())
}

fn note_on_synth(voices: &mut [SynthVoice], event: &MidiEvent) -> Result<(), PlaybackError> {
    if event.kind == MidiKind::Off || event.velocity == 0 {
        release_synth(voices, event.note);
        return Ok(());
    }
    if let Some(voice) = voices
        .iter_mut()
        .find(|voice| voice.inner.is_active() && voice.note == Some(event.note))
    {
        voice.inner.note_on(event.pitch, event.velocity);
        return Ok(());
    }
    let Some(slot) = voices.iter_mut().find(|voice| !voice.inner.is_active()) else {
        return Err(PlaybackError::BufferTooSmall);
    };
    slot.note = Some(event.note);
    slot.inner.note_on(event.pitch, event.velocity);
    Ok(())
}

fn note_on_sampler(
    voices: &mut [SamplerVoice],
    event: &MidiEvent,
    root: u8,
) -> Result<(), PlaybackError> {
    if event.kind == MidiKind::Off || event.velocity == 0 {
        release_sampler(voices, event.note);
        return Ok(());
    }
    let started = armed_sampler(event.note, event.pitch, event.velocity, root);
    if let Some(voice) = voices
        .iter_mut()
        .find(|voice| voice.active && voice.note == Some(event.note))
    {
        *voice = started;
        return Ok(());
    }
    let Some(slot) = voices.iter_mut().find(|voice| !voice.active) else {
        return Err(PlaybackError::BufferTooSmall);
    };
    *slot = started;
    Ok(())
}

fn armed_sampler(note: Id, pitch: u8, velocity: u8, root: u8) -> SamplerVoice {
    SamplerVoice {
        note: Some(note),
        position: 0,
        step: sample_step(pitch, root),
        gain: f32::from(velocity) / 127.0,
        active: true,
        fed: 0,
        sinc: fresh_sinc(),
    }
}

fn release_synth(voices: &mut [SynthVoice], note: Id) {
    for voice in voices.iter_mut() {
        if voice.inner.is_active() && voice.note == Some(note) {
            voice.inner.note_off();
        }
    }
}

fn release_sampler(voices: &mut [SamplerVoice], note: Id) {
    for voice in voices.iter_mut() {
        if voice.active && voice.note == Some(note) {
            voice.active = false;
        }
    }
}

fn mix_synth(voices: &mut [SynthVoice]) -> f32 {
    let mut sum = 0.0;
    for voice in voices.iter_mut() {
        sum += voice.inner.next_sample();
    }
    sum
}

fn mix_sampler(
    voices: &mut [SamplerVoice],
    pcm: &[f32],
    interpolation: SamplerInterpolation,
) -> f32 {
    let mut sum = 0.0;
    for voice in voices.iter_mut() {
        if !voice.active {
            continue;
        }
        let index = usize::try_from(voice.position >> 32).unwrap_or(usize::MAX);
        if index >= pcm.len() {
            voice.active = false;
            continue;
        }
        let fraction = (voice.position & 0xffff_ffff) as f64 / 4_294_967_296.0;
        let sample = match interpolation {
            SamplerInterpolation::Floor => Floor::new(pcm[index]).interpolate(fraction),
            SamplerInterpolation::Linear => {
                let next = pcm.get(index + 1).copied().unwrap_or(0.0);
                Linear::new(pcm[index], next).interpolate(fraction)
            }
            SamplerInterpolation::Sinc => {
                while voice.fed <= u64::try_from(index).unwrap_or(u64::MAX) {
                    let frame = pcm
                        .get(usize::try_from(voice.fed).unwrap_or(usize::MAX))
                        .copied()
                        .unwrap_or(0.0);
                    voice.sinc.next_source_frame(frame);
                    voice.fed += 1;
                }
                voice.sinc.interpolate(fraction)
            }
        };
        sum += sample * voice.gain;
        voice.position = voice.position.saturating_add(voice.step);
    }
    sum
}

fn sample_step(pitch: u8, root: u8) -> u64 {
    let semis = f64::from(i16::from(pitch) - i16::from(root));
    let rate = 2f64.powf(semis / 12.0);
    let step = rate * 4_294_967_296.0;
    if !step.is_finite() || step <= 0.0 {
        return 0;
    }
    step.round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::MidiKind;

    fn edge(offset: u32, note: u128, pitch: u8, velocity: u8, kind: MidiKind) -> MidiEvent {
        MidiEvent {
            sample_offset: offset,
            track: Id::from_raw(1),
            note: Id::from_raw(note),
            pitch,
            velocity,
            kind,
        }
    }

    fn on(offset: u32, note: u128, pitch: u8) -> MidiEvent {
        edge(offset, note, pitch, 127, MidiKind::On)
    }

    fn off(offset: u32, note: u128) -> MidiEvent {
        edge(offset, note, 60, 0, MidiKind::Off)
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
}
