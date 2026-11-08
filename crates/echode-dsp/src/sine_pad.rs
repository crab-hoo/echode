//! The pinned wavesynth preset, without any host, device, or transport dependency.

use echode_realtime::Event;
use wavesynth::engine::voice::Voice;
use wavesynth::instrument::Instrument;

use crate::{DspError, ForbidAlloc};

pub const SYNTH_ID: &str = "wavesynth";
pub const SYNTH_VERSION: &str = "0.1.0";
pub const INSTRUMENT: &str = "SinePad";
const STATE_VERSION: u8 = 1;
const RELEASE_MS: u64 = 400;

/// One wavesynth voice, built for a single sample rate.
pub struct SynthVoice {
    inner: Voice,
    sample_rate: u32,
    note_id: Option<u32>,
    releasing: bool,
}

impl SynthVoice {
    pub fn new(sample_rate: u32) -> Result<Self, DspError> {
        if sample_rate == 0 {
            return Err(DspError::InvalidSampleRate);
        }
        Ok(Self {
            inner: Voice::new(sample_rate as f32, Instrument::SinePad),
            sample_rate,
            note_id: None,
            releasing: false,
        })
    }

    pub fn is_active(&self) -> bool {
        self.inner.is_active()
    }
}

/// Sine Pad DSP. Prepare voices off the audio thread; process never allocates.
pub struct SinePad {
    voices: Vec<SynthVoice>,
    sample_rate: u32,
}

impl SinePad {
    pub fn new(sample_rate: u32) -> Result<Self, DspError> {
        if sample_rate == 0 {
            return Err(DspError::InvalidSampleRate);
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

    pub fn prepare(&mut self, voices: usize, sample_rate: u32) -> Result<(), DspError> {
        if sample_rate == 0 {
            return Err(DspError::InvalidSampleRate);
        }
        self.voices.clear();
        self.voices.reserve(voices.max(1));
        self.sample_rate = sample_rate;
        Ok(())
    }

    pub fn process(
        &mut self,
        planes: &mut [&mut [f32]],
        events: &[Event],
        sample_rate: u32,
    ) -> Result<(), DspError> {
        let _guard = ForbidAlloc::enter();
        if sample_rate == 0 || sample_rate != self.sample_rate {
            return Err(DspError::InvalidSampleRate);
        }
        let Some((first, rest)) = planes.split_first_mut() else {
            return Ok(());
        };
        let frames = first.len();
        if rest.iter().any(|plane| plane.len() != frames) {
            return Err(DspError::BufferTooSmall);
        }
        // Capacity can cover a long score. Walk only sounding voices, not the
        // reserved slots for future notes.
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
                        return Err(DspError::BufferTooSmall);
                    }
                    synth.voices.push(SynthVoice::new(sample_rate)?);
                }
                note_on(&mut synth.voices, event)
            },
            |synth| {
                let sample = mix(&mut synth.voices);
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

    /// Preserve the original engine fixture's version-one state bytes.
    pub fn save(&self, out: &mut Vec<u8>) {
        out.push(STATE_VERSION);
        out.extend_from_slice(INSTRUMENT.as_bytes());
    }

    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), DspError> {
        if bytes.split_first() == Some((&STATE_VERSION, INSTRUMENT.as_bytes())) {
            Ok(())
        } else {
            Err(DspError::InvalidState)
        }
    }

    pub fn latency_samples(&self) -> u32 {
        0
    }

    pub fn tail_samples(&self, sample_rate: u32) -> u32 {
        u32::try_from(u64::from(sample_rate) * RELEASE_MS / 1_000).unwrap_or(0)
    }
}

/// Adds Sine Pad to the output using caller-owned voice scratch, without allocation.
pub fn render_sine(
    events: &[Event],
    voices: &mut [SynthVoice],
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), DspError> {
    if sample_rate == 0 || voices.iter().any(|voice| voice.sample_rate != sample_rate) {
        return Err(DspError::InvalidSampleRate);
    }
    let _guard = ForbidAlloc::enter();
    drive(events, output, voices, note_on, mix)
}

fn drive<T: ?Sized>(
    events: &[Event],
    output: &mut [f32],
    state: &mut T,
    mut handle: impl FnMut(&mut T, &Event) -> Result<(), DspError>,
    mut mix: impl FnMut(&mut T) -> f32,
) -> Result<(), DspError> {
    let mut previous = None;
    for event in events {
        let offset = usize::try_from(event.time()).map_err(|_| DspError::Time)?;
        if offset >= output.len() {
            return Err(DspError::BufferTooSmall);
        }
        if previous.is_some_and(|previous| offset < previous) {
            return Err(DspError::Time);
        }
        previous = Some(offset);
    }
    let mut events = events.iter().peekable();
    for (frame, sample) in output.iter_mut().enumerate() {
        let at = u32::try_from(frame).map_err(|_| DspError::Time)?;
        while let Some(event) = events.next_if(|event| event.time() == at) {
            handle(state, event)?;
        }
        *sample += mix(state);
    }
    Ok(())
}

fn note_on(voices: &mut [SynthVoice], event: &Event) -> Result<(), DspError> {
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
        for voice in voices.iter_mut() {
            if voice.inner.is_active() && voice.note_id == Some(note_id) {
                voice.inner.note_off();
                voice.releasing = true;
            }
        }
        return Ok(());
    }
    // Preserve the original 16-bit storage quantization and top-seven-bit
    // conversion before feeding wavesynth's MIDI velocity.
    let stored = (note.velocity.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let velocity = (stored >> 9) as u8;
    if let Some(voice) = voices
        .iter_mut()
        .find(|voice| voice.inner.is_active() && voice.note_id == Some(note_id))
    {
        voice.inner.note_on(key, velocity);
        voice.releasing = false;
        return Ok(());
    }
    let Some(slot) = voices.iter_mut().find(|voice| !voice.inner.is_active()) else {
        return Err(DspError::BufferTooSmall);
    };
    slot.note_id = Some(note_id);
    slot.releasing = false;
    slot.inner.note_on(key, velocity);
    Ok(())
}

fn mix(voices: &mut [SynthVoice]) -> f32 {
    let mut sum = 0.0;
    for voice in voices.iter_mut() {
        if voice.inner.is_active() {
            sum += voice.inner.next_sample();
        }
    }
    sum
}

#[cfg(test)]
#[path = "sine_pad_tests.rs"]
mod tests;
