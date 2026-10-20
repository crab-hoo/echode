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
use wavesynth::engine::voice::Voice;
use wavesynth::instrument::Instrument;

use crate::realtime::ForbidAlloc;
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
    Floor = 0,
    #[default]
    Linear = 1,
    Sinc = 2,
}

/// Ring length for dasp's sinc. Depth is half of this. The window belongs to
/// the interpolator; it is not a limit on how long a recording may be.
const SINC_FRAMES: usize = 16;

/// One wavesynth voice, built for a single sample rate.
pub struct SynthVoice {
    inner: Voice,
    sample_rate: u32,
    note_id: Option<u32>,
    releasing: bool,
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
            note_id: None,
            releasing: false,
        })
    }
}

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
const SINE_PAD_RELEASE_MS: u64 = 400;

/// Test synth. Voices are sized by [`Device::prepare`], not during `process`.
pub struct TestSynth {
    voices: Vec<SynthVoice>,
    sample_rate: u32,
}

impl TestSynth {
    pub fn new(sample_rate: u32) -> Result<Self, PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        Ok(Self {
            voices: Vec::new(),
            sample_rate,
        })
    }

    pub fn silence(&mut self) {
        self.voices.clear();
    }

    /// Transfers only active DSP voices into prepared storage, without allocation.
    pub fn carry_from(&mut self, old: &mut Self, covers: impl Fn(u32) -> Option<bool>) {
        old.voices.retain_mut(|old| {
            let Some(held) = old.note_id.and_then(&covers) else {
                return true;
            };
            if self.voices.len() == self.voices.capacity() {
                return true;
            }
            let fresh = SynthVoice {
                inner: Voice::new(self.sample_rate as f32, Instrument::SinePad),
                sample_rate: self.sample_rate,
                note_id: None,
                releasing: false,
            };
            let mut voice = std::mem::replace(old, fresh);
            if !held && !voice.releasing {
                voice.inner.note_off();
                voice.releasing = true;
            }
            self.voices.push(voice);
            false
        });
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
    let mut index = 0;
    for (frame, sample) in output.iter_mut().enumerate() {
        let at = u32::try_from(frame).map_err(|_| PlaybackError::Time)?;
        while index < events.len() && events[index].time() == at {
            handle(state, &events[index])?;
            index += 1;
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

fn note_on_synth(voices: &mut [SynthVoice], event: &Event) -> Result<(), PlaybackError> {
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
        release_synth(voices, note_id);
        return Ok(());
    }
    let velocity = midi7(note.velocity);
    if let Some(voice) = voices
        .iter_mut()
        .find(|voice| voice.inner.is_active() && voice.note_id == Some(note_id))
    {
        voice.inner.note_on(key, velocity);
        voice.releasing = false;
        return Ok(());
    }
    let Some(slot) = voices.iter_mut().find(|voice| !voice.inner.is_active()) else {
        return Err(PlaybackError::BufferTooSmall);
    };
    slot.note_id = Some(note_id);
    slot.releasing = false;
    slot.inner.note_on(key, velocity);
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

fn release_synth(voices: &mut [SynthVoice], note_id: u32) {
    for voice in voices.iter_mut() {
        if voice.inner.is_active() && voice.note_id == Some(note_id) {
            voice.inner.note_off();
            voice.releasing = true;
        }
    }
}

fn release_sampler(voices: &mut [SamplerVoice], note_id: u32) {
    for voice in voices.iter_mut() {
        if voice.active && voice.note_id == Some(note_id) {
            voice.active = false;
        }
    }
}

fn mix_synth(voices: &mut [SynthVoice]) -> f32 {
    let mut sum = 0.0;
    for voice in voices.iter_mut() {
        if voice.inner.is_active() {
            sum += voice.inner.next_sample();
        }
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

impl crate::builtin::Device for TestSynth {
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
        let Some((first, rest)) = planes.split_first_mut() else {
            return Ok(());
        };
        let frames = first.len();
        if rest.iter().any(|plane| plane.len() != frames) {
            return Err(PlaybackError::BufferTooSmall);
        }
        // Reserved capacity can cover a long score. Walk only sounding voices,
        // so callback work never scales with the number of future notes.
        drive(
            events,
            first,
            self,
            |synth, event| {
                if event.is_note_on()
                    && event.note_id().is_some()
                    && !synth
                        .voices
                        .iter()
                        .any(|voice| voice.note_id == event.note_id())
                {
                    if synth.voices.len() == synth.voices.capacity() {
                        return Err(PlaybackError::BufferTooSmall);
                    }
                    synth.voices.push(SynthVoice::new(sample_rate)?);
                }
                note_on_synth(&mut synth.voices, event)
            },
            |synth| {
                let sample = mix_synth(&mut synth.voices);
                synth.voices.retain(|voice| voice.inner.is_active());
                sample
            },
        )?;
        for plane in rest.iter_mut() {
            for (sample, source) in plane.iter_mut().zip(first.iter()) {
                *sample += *source;
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
        self.sample_rate = sample_rate;
        Ok(())
    }

    fn save(&self, out: &mut Vec<u8>) {
        out.push(STATE_VERSION);
        out.extend_from_slice(TEST_SYNTH_INSTRUMENT.as_bytes());
    }

    fn restore(&mut self, bytes: &[u8]) -> Result<(), PlaybackError> {
        if bytes.first() == Some(&STATE_VERSION) && &bytes[1..] == TEST_SYNTH_INSTRUMENT.as_bytes()
        {
            Ok(())
        } else {
            Err(PlaybackError::InvalidDevice)
        }
    }

    fn latency_samples(&self) -> u32 {
        0
    }

    fn tail_samples(&self, sample_rate: u32) -> u32 {
        sine_pad_tail(sample_rate).unwrap_or(0)
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
        let mut index = 0;
        for frame in 0..frames {
            let at = u32::try_from(frame).map_err(|_| PlaybackError::Time)?;
            while index < events.len() && events[index].time() == at {
                self.apply(&events[index])?;
                index += 1;
            }
            let sample = mix_sampler(&mut self.voices, self.pcm, self.interpolation);
            for plane in planes.iter_mut() {
                plane[frame] += sample;
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

fn sine_pad_tail(sample_rate: u32) -> Result<u32, PlaybackError> {
    u32::try_from(u64::from(sample_rate) * SINE_PAD_RELEASE_MS / 1_000)
        .map_err(|_| PlaybackError::Time)
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
