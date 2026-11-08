//! One raw sample at a time, heard beside the project.
//!
//! The control side decodes, converts and allocates. The callback only takes
//! prepared sounds from a ring, adds their samples to the block and hands each
//! sound back for the control side to free. A newer sound replaces the one
//! being heard: the old one fades out over 5 ms while the new one starts at its
//! first sample. Positions advance per sample, so the result does not depend
//! on how the device cuts its blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use dasp_interpolate::{Interpolator, linear::Linear};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::PlaybackError;
use crate::declick::stop_gain;
use crate::realtime::ForbidAlloc;

/// Sounds handed to the callback and not yet returned. Both rings are sized
/// from it, so a return never fails. A newer request waits on the control side
/// while this many are out, replacing any older waiting request. It bounds
/// this handoff only; it is not a limit on files, tracks or channels.
const IN_FLIGHT: usize = 4;

/// Resident device-rate planes or a bounded stream of prepared stereo frames.
/// Resident mono has no `right` and is heard unchanged on both channels.
#[derive(Debug)]
pub struct AuditionSound {
    left: Arc<[f32]>,
    right: Option<Arc<[f32]>>,
    stream: Option<Consumer<[f32; 2]>>,
    finished: Arc<AtomicBool>,
}

/// The control-side writer of a bounded PCM ring. Dropping it marks EOF;
/// the callback drains the queued frames before ending.
pub struct AuditionWriter {
    frames: Producer<[f32; 2]>,
    finished: Arc<AtomicBool>,
}

impl AuditionWriter {
    /// False means backpressure; retry this frame after servicing the output.
    pub fn push(&mut self, frame: [f32; 2]) -> Result<bool, PlaybackError> {
        if frame.iter().any(|value| !value.is_finite()) {
            return Err(PlaybackError::Media);
        }
        Ok(self.frames.push(frame).is_ok())
    }
}

impl Drop for AuditionWriter {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
    }
}

impl AuditionSound {
    pub fn new(left: Arc<[f32]>, right: Option<Arc<[f32]>>) -> Result<Self, PlaybackError> {
        let samples = left
            .iter()
            .chain(right.iter().flat_map(|right| right.iter()));
        if left.is_empty()
            || right
                .as_ref()
                .is_some_and(|right| right.len() != left.len())
            || samples.copied().any(|sample| !sample.is_finite())
        {
            return Err(PlaybackError::Media);
        }
        Ok(Self {
            left,
            right,
            stream: None,
            finished: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Allocates only the read-ahead window, irrespective of file length.
    /// `capacity` bounds this handoff, never the duration of the sound.
    pub fn streaming(capacity: usize) -> Result<(AuditionWriter, Self), PlaybackError> {
        if capacity == 0 {
            return Err(PlaybackError::Media);
        }
        let (frames, stream) = RingBuffer::new(capacity);
        let finished = Arc::new(AtomicBool::new(false));
        Ok((
            AuditionWriter {
                frames,
                finished: Arc::clone(&finished),
            },
            Self {
                left: Arc::from([]),
                right: None,
                stream: Some(stream),
                finished,
            },
        ))
    }

    /// Resident whole-file frames; streaming PCM lives in the handoff ring.
    pub fn frames(&self) -> usize {
        self.left.len()
    }
}

enum Message {
    Play { id: u64, sound: AuditionSound },
    Stop,
}

#[derive(Default)]
struct Shared {
    /// The sound being heard from its start, or 0. A fading sound is not.
    heard: AtomicU64,
    /// The newest sound that reached its end, faded out or was replaced unheard.
    ended: AtomicU64,
    starved_frames: AtomicU64,
    gain: AtomicU32,
}

/// The control half. It owns every allocation and free.
pub struct AuditionControl {
    commands: Producer<Message>,
    retired: Consumer<AuditionSound>,
    waiting: Option<Message>,
    in_flight: usize,
    next_id: u64,
    shared: Arc<Shared>,
}

/// The callback half. [`AuditionAudio::mix`] adds to the caller's buffers.
pub struct AuditionAudio {
    commands: Consumer<Message>,
    retired: Producer<AuditionSound>,
    current: Option<Voice>,
    fading: Option<Voice>,
    fade_frames: u32,
    shared: Arc<Shared>,
    started: u64,
    gain: Gain,
}

/// Both voices use the same sample-counted ramp, without scaling the caller's
/// existing mix. Interpolation matches the project's fader implementation.
struct Gain {
    current: f32,
    start: f32,
    target: f32,
    elapsed: u32,
    frames: u32,
}

impl Gain {
    fn at(&self, offset: usize) -> f32 {
        let elapsed = self
            .elapsed
            .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
        if elapsed >= self.frames {
            self.target
        } else {
            Linear::new(self.start, self.target)
                .interpolate(f64::from(elapsed) / f64::from(self.frames))
        }
    }

    fn retarget(&mut self, target: f32, silent: bool) {
        if self.target != target {
            self.start = self.current;
            self.target = target;
            self.elapsed = 0;
        }
        if silent {
            self.current = target;
            self.start = target;
            self.elapsed = self.frames;
        }
    }

    fn advance(&mut self, frames: usize) {
        self.current = self.at(frames);
        self.elapsed = self
            .elapsed
            .saturating_add(u32::try_from(frames).unwrap_or(u32::MAX))
            .min(self.frames);
    }
}

struct Voice {
    id: u64,
    sound: AuditionSound,
    position: usize,
    /// Samples since the fade began. Only a fading voice uses it.
    age: u64,
    starved_frames: u64,
}

/// The fade matches the live stop: 5 ms at `sample_rate`.
pub fn audition_pair(sample_rate: u32) -> Result<(AuditionControl, AuditionAudio), PlaybackError> {
    if sample_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    // Stops carry no sound. Twice the sounds leaves them room as well, and a
    // full ring only keeps the newest request waiting on the control side.
    let (commands, incoming) = RingBuffer::new(IN_FLIGHT * 2);
    let (retired, returned) = RingBuffer::new(IN_FLIGHT);
    let shared = Arc::new(Shared {
        gain: AtomicU32::new(1.0f32.to_bits()),
        ..Shared::default()
    });
    Ok((
        AuditionControl {
            commands,
            retired: returned,
            waiting: None,
            in_flight: 0,
            next_id: 0,
            shared: Arc::clone(&shared),
        },
        AuditionAudio {
            commands: incoming,
            retired,
            current: None,
            fading: None,
            fade_frames: (sample_rate / 200).max(2),
            shared,
            started: 0,
            gain: Gain {
                current: 1.0,
                start: 1.0,
                target: 1.0,
                elapsed: 0,
                frames: (sample_rate / 200).max(1),
            },
        },
    ))
}

impl AuditionControl {
    /// Attenuates only this audition, with a 5 ms ramp while sounding.
    pub fn set_gain(&self, gain: f32) -> Result<(), PlaybackError> {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return Err(PlaybackError::Media);
        }
        self.shared.gain.store(gain.to_bits(), Ordering::Release);
        Ok(())
    }
    /// Replaces whatever is heard with `sound`. Returns the id it reports.
    pub fn play(&mut self, sound: AuditionSound) -> u64 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.waiting = Some(Message::Play { id, sound });
        self.service();
        id
    }

    /// Fades out the heard sound. A sound still waiting here is dropped.
    pub fn stop(&mut self) {
        self.waiting = Some(Message::Stop);
        self.service();
    }

    /// Frees returned sounds, then sends the waiting request if it fits.
    pub fn service(&mut self) {
        while let Ok(sound) = self.retired.pop() {
            drop(sound);
            self.in_flight = self.in_flight.saturating_sub(1);
        }
        let fits = match &self.waiting {
            Some(Message::Play { .. }) => self.in_flight < IN_FLIGHT,
            Some(Message::Stop) => true,
            None => false,
        };
        if !fits {
            return;
        }
        if let Some(message) = self.waiting.take() {
            let carries = matches!(message, Message::Play { .. });
            match self.commands.push(message) {
                Ok(()) if carries => self.in_flight += 1,
                Ok(()) => {}
                Err(PushError::Full(message)) => self.waiting = Some(message),
            }
        }
    }

    /// The id being heard from its start, if any.
    pub fn heard(&self) -> Option<u64> {
        Some(self.shared.heard.load(Ordering::Acquire)).filter(|id| *id != 0)
    }

    /// The newest id that played to its end, faded out or was replaced unheard.
    pub fn ended(&self) -> u64 {
        self.shared.ended.load(Ordering::Acquire)
    }

    /// Output frames silenced by a temporarily empty stream, for the latest sound.
    pub fn starved_frames(&self) -> u64 {
        self.shared.starved_frames.load(Ordering::Acquire)
    }

    /// Nothing is waiting, sounding, fading or left to free.
    pub fn idle(&self) -> bool {
        self.waiting.is_none() && self.in_flight == 0
    }
}

impl AuditionAudio {
    /// Adds the heard and fading sounds to `left` and `right`. Messages are
    /// adopted at the start of the block. Never allocates or frees.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn mix(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _guard = ForbidAlloc::enter();
        self.gain.retarget(
            f32::from_bits(self.shared.gain.load(Ordering::Acquire)),
            self.current.is_none() && self.fading.is_none(),
        );
        while let Ok(message) = self.commands.pop() {
            self.release_current();
            if let Message::Play { id, sound } = message {
                self.started = id;
                self.shared.starved_frames.store(0, Ordering::Release);
                self.current = Some(Voice {
                    id,
                    sound,
                    position: 0,
                    age: 0,
                    starved_frames: 0,
                });
            }
        }
        let frames = left.len().min(right.len());
        let (Some(left), Some(right)) = (left.get_mut(..frames), right.get_mut(..frames)) else {
            return;
        };
        let fade_frames = self.fade_frames;
        if let Some(voice) = &mut self.fading
            && voice.add_fading(left, right, fade_frames, &self.gain)
            && let Some(voice) = self.fading.take()
        {
            self.retire(voice);
        }
        if let Some(voice) = &mut self.current {
            let ended = voice.add(left, right, &self.gain);
            self.shared
                .starved_frames
                .store(voice.starved_frames, Ordering::Release);
            if ended && let Some(voice) = self.current.take() {
                self.retire(voice);
            }
        }
        let heard = self.current.as_ref().map_or(0, |voice| voice.id);
        self.shared.heard.store(heard, Ordering::Release);
        self.gain.advance(frames);
    }

    /// The newest sound adopted so far, or 0. It started at the first frame
    /// of the block whose [`AuditionAudio::mix`] adopted it, even if it
    /// already ended there.
    pub fn started(&self) -> u64 {
        self.started
    }

    fn release_current(&mut self) {
        let Some(voice) = self.current.take() else {
            return;
        };
        if voice.position == 0 {
            // Replaced before a sample was heard.
            self.retire(voice);
        } else if let Some(older) = self.fading.replace(voice) {
            // Two replacements within one fade cut the older, partly faded sound.
            self.retire(older);
        }
    }

    fn retire(&mut self, voice: Voice) {
        // Ids only grow, so a fade that outlasts a newer, shorter sound keeps it ended.
        self.shared.ended.fetch_max(voice.id, Ordering::AcqRel);
        if let Err(PushError::Full(sound)) = self.retired.push(voice.sound) {
            // The in-flight count makes this unreachable. Leaking keeps a free
            // off this thread if that accounting is ever broken.
            std::mem::forget(sound);
        }
    }
}

impl Voice {
    fn next(&mut self) -> Option<[f32; 2]> {
        let frame = if let Some(stream) = &mut self.sound.stream {
            stream.pop().ok()
        } else {
            self.sound.left.get(self.position).copied().map(|left| {
                [
                    left,
                    self.sound
                        .right
                        .as_ref()
                        .and_then(|right| right.get(self.position))
                        .copied()
                        .unwrap_or(left),
                ]
            })
        };
        if frame.is_some() {
            self.position = self.position.saturating_add(1);
        }
        frame
    }

    fn ended(&self) -> bool {
        if let Some(stream) = &self.sound.stream {
            self.sound.finished.load(Ordering::Acquire) && stream.is_empty()
        } else {
            self.position >= self.sound.frames()
        }
    }

    /// True once EOF has been published and every queued sample was heard.
    fn add(&mut self, left: &mut [f32], right: &mut [f32], gain: &Gain) -> bool {
        let frames = left.len().min(right.len());
        for (index, (left, right)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let Some([l, r]) = self.next() else {
                if !self.ended() {
                    self.starved_frames =
                        self.starved_frames.saturating_add((frames - index) as u64);
                }
                break;
            };
            *left += l * gain.at(index + 1);
            *right += r * gain.at(index + 1);
        }
        self.ended()
    }

    /// The fade advances in output samples, including any temporarily empty ring.
    fn add_fading(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        fade_frames: u32,
        volume: &Gain,
    ) -> bool {
        for (index, (left, right)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let gain = stop_gain(self.age, fade_frames);
            if gain == 0.0 {
                break;
            }
            if let Some([l, r]) = self.next() {
                *left += l * gain * volume.at(index + 1);
                *right += r * gain * volume.at(index + 1);
            }
            self.age += 1;
        }
        self.ended() || stop_gain(self.age, fade_frames) == 0.0
    }
}

#[cfg(test)]
#[path = "audition_tests.rs"]
mod tests;
