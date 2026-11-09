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
use echode_realtime::Event;

use crate::PlaybackError;
use crate::realtime::ForbidAlloc;

pub use echode_dsp::{
    INSTRUMENT as TEST_SYNTH_INSTRUMENT, SYNTH_ID as TEST_SYNTH_ID,
    SYNTH_VERSION as TEST_SYNTH_VERSION, SynthVoice,
};
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
    Floor = 0,
    #[default]
    Linear = 1,
    Sinc = 2,
}

/// Ring length for dasp's sinc. Depth is half of this. The window belongs to
/// the interpolator; it is not a limit on how long a recording may be.
const SINC_FRAMES: usize = 16;

/// One sampler voice. The sinc window is only used for [`SamplerInterpolation::Sinc`].
pub struct SamplerVoice {
    note_id: Option<u32>,
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
            note_id: None,
            position: 0,
            step: 0,
            gain: 0.0,
            active: false,
            fed: 0,
            sinc: fresh_sinc(),
        }
    }
}

/// Parameter id for the test sampler's interpolator. 0 is floor, 1 is linear, 2 is sinc.
pub const SAMPLER_INTERPOLATION: u32 = 0;

const STATE_VERSION: u8 = 1;

/// Engine fixture adapter for the host-independent Sine Pad DSP.
pub struct TestSynth(echode_dsp::SinePad);

impl TestSynth {
    pub fn new(sample_rate: u32) -> Result<Self, PlaybackError> {
        Ok(Self(echode_dsp::SinePad::new(sample_rate)?))
    }

    pub fn silence(&mut self) {
        self.0.silence();
    }

    pub fn carry_from(&mut self, old: &mut Self, covers: impl Fn(u32) -> Option<bool>) {
        self.0.carry_from(&mut old.0, covers);
    }
}

/// Test sampler. `pcm` stays with the caller and is not part of the saved state.
pub struct TestSampler<'a> {
    pcm: &'a [f32],
    root: u8,
    interpolation: SamplerInterpolation,
    voices: Vec<SamplerVoice>,
    sample_rate: u32,
}

impl<'a> TestSampler<'a> {
    pub fn new(pcm: &'a [f32], root: u8, sample_rate: u32) -> Result<Self, PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        Ok(Self {
            pcm,
            root,
            interpolation: SamplerInterpolation::Linear,
            voices: Vec::new(),
            sample_rate,
        })
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
    events: &[Event],
    voices: &mut [SynthVoice],
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    echode_dsp::render_sine(events, voices, sample_rate, output).map_err(Into::into)
}

/// Adds a one-shot sampler into `output`.
///
/// `pcm` is already at the device rate. `root` is the pitch that plays it at
/// its stored speed. Playback stops at the end of `pcm` and on note-off.
pub fn render_test_sampler(
    events: &[Event],
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
    events: &[Event],
    output: &mut [f32],
    state: &mut T,
    mut handle: impl FnMut(&mut T, &Event) -> Result<(), PlaybackError>,
    mut mix: impl FnMut(&mut T) -> f32,
) -> Result<(), PlaybackError> {
    check_events(events, output.len())?;
    let mut events = events.iter().peekable();
    for (frame, sample) in output.iter_mut().enumerate() {
        let at = u32::try_from(frame).map_err(|_| PlaybackError::Time)?;
        while let Some(event) = events.next_if(|event| event.time() == at) {
            handle(state, event)?;
        }
        *sample += mix(state);
    }
    Ok(())
}

fn check_events(events: &[Event], frames: usize) -> Result<(), PlaybackError> {
    let mut previous = None;
    for event in events {
        let offset = usize::try_from(event.time()).map_err(|_| PlaybackError::Time)?;
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

fn midi7(velocity: f64) -> u8 {
    let stored = (velocity.clamp(0.0, 1.0) * 65535.0).round() as u16;
    echode_model::velocity_to_midi7(stored)
}

fn note_on_sampler(
    voices: &mut [SamplerVoice],
    event: &Event,
    root: u8,
) -> Result<(), PlaybackError> {
    let (Event::NoteOff(note) | Event::NoteOn(note)) = event else {
        return Ok(());
    };
    let Some(note_id) = note.target.note_id else {
        return Ok(());
    };
    let Some(key) = note.target.key else {
        return Ok(());
    };
    if matches!(event, Event::NoteOff(_)) {
        release_sampler(voices, note_id);
        return Ok(());
    }
    let started = armed_sampler(note_id, key, midi7(note.velocity), root);
    if let Some(voice) = voices
        .iter_mut()
        .find(|voice| voice.active && voice.note_id == Some(note_id))
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

fn armed_sampler(note_id: u32, pitch: u8, velocity: u8, root: u8) -> SamplerVoice {
    SamplerVoice {
        note_id: Some(note_id),
        position: 0,
        step: sample_step(pitch, root),
        gain: f32::from(velocity) / 127.0,
        active: true,
        fed: 0,
        sinc: fresh_sinc(),
    }
}

fn release_sampler(voices: &mut [SamplerVoice], note_id: u32) {
    for voice in voices.iter_mut() {
        if voice.active && voice.note_id == Some(note_id) {
            voice.active = false;
        }
    }
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
        let Some(&current) = pcm.get(index) else {
            voice.active = false;
            continue;
        };
        let fraction = (voice.position & 0xffff_ffff) as f64 / 4_294_967_296.0;
        let sample = match interpolation {
            SamplerInterpolation::Floor => Floor::new(current).interpolate(fraction),
            SamplerInterpolation::Linear => {
                let next = pcm.get(index + 1).copied().unwrap_or(0.0);
                Linear::new(current, next).interpolate(fraction)
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

impl crate::builtin::Device for TestSynth {
    fn process(
        &mut self,
        planes: &mut [&mut [f32]],
        events: &[Event],
        sample_rate: u32,
    ) -> Result<(), PlaybackError> {
        self.0
            .process(planes, events, sample_rate)
            .map_err(Into::into)
    }

    fn prepare(&mut self, voices: usize, sample_rate: u32) -> Result<(), PlaybackError> {
        self.0.prepare(voices, sample_rate).map_err(Into::into)
    }

    fn save(&self, out: &mut Vec<u8>) {
        self.0.save(out);
    }

    fn restore(&mut self, bytes: &[u8]) -> Result<(), PlaybackError> {
        self.0.restore(bytes).map_err(Into::into)
    }

    fn latency_samples(&self) -> u32 {
        self.0.latency_samples()
    }

    fn tail_samples(&self, sample_rate: u32) -> u32 {
        self.0.tail_samples(sample_rate)
    }
}

impl crate::builtin::Device for TestSampler<'_> {
    fn process(
        &mut self,
        planes: &mut [&mut [f32]],
        events: &[Event],
        sample_rate: u32,
    ) -> Result<(), PlaybackError> {
        let _guard = crate::realtime::ForbidAlloc::enter();
        if sample_rate == 0 || sample_rate != self.sample_rate {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let frames = match planes.first() {
            Some(plane) => plane.len(),
            None => return Ok(()),
        };
        if planes.iter().any(|plane| plane.len() != frames) {
            return Err(PlaybackError::BufferTooSmall);
        }
        check_events(events, frames)?;
        let mut events = events.iter().peekable();
        for frame in 0..frames {
            let at = u32::try_from(frame).map_err(|_| PlaybackError::Time)?;
            while let Some(event) = events.next_if(|event| event.time() == at) {
                self.apply(event)?;
            }
            let sample = mix_sampler(&mut self.voices, self.pcm, self.interpolation);
            for value in planes.iter_mut().filter_map(|plane| plane.get_mut(frame)) {
                *value += sample;
            }
        }
        Ok(())
    }

    fn prepare(&mut self, voices: usize, sample_rate: u32) -> Result<(), PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let count = voices.max(1);
        self.voices.clear();
        self.voices.reserve(count);
        self.voices.resize_with(count, SamplerVoice::idle);
        self.sample_rate = sample_rate;
        Ok(())
    }

    fn save(&self, out: &mut Vec<u8>) {
        out.push(STATE_VERSION);
        out.push(self.interpolation as u8);
    }

    fn restore(&mut self, bytes: &[u8]) -> Result<(), PlaybackError> {
        let &[STATE_VERSION, code] = bytes else {
            return Err(PlaybackError::InvalidDevice);
        };
        self.interpolation = interpolation_from_code(code)?;
        Ok(())
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self, _sample_rate: u32) -> u32 {
        0
    }
}

impl TestSampler<'_> {
    fn apply(&mut self, event: &Event) -> Result<(), PlaybackError> {
        if let Event::ParamValue(param) = event
            && param.param_id == SAMPLER_INTERPOLATION
        {
            self.interpolation = interpolation_from_value(param.value)?;
            return Ok(());
        }
        note_on_sampler(&mut self.voices, event, self.root)
    }
}

fn interpolation_from_code(code: u8) -> Result<SamplerInterpolation, PlaybackError> {
    match code {
        0 => Ok(SamplerInterpolation::Floor),
        1 => Ok(SamplerInterpolation::Linear),
        2 => Ok(SamplerInterpolation::Sinc),
        _ => Err(PlaybackError::InvalidDevice),
    }
}

fn interpolation_from_value(value: f64) -> Result<SamplerInterpolation, PlaybackError> {
    if value == 0.0 {
        Ok(SamplerInterpolation::Floor)
    } else if value == 1.0 {
        Ok(SamplerInterpolation::Linear)
    } else if value == 2.0 {
        Ok(SamplerInterpolation::Sinc)
    } else {
        Err(PlaybackError::InvalidDevice)
    }
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
#[path = "synth_tests.rs"]
mod tests;
