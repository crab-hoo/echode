//! Play position in device samples.
//!
//! One call to [`Transport::pull`] moves the playhead and reports the sample
//! ranges the block covers. A loop produces at most one extra range, the piece
//! after the playhead jumps back. The caller supplies the range buffer.

use echode_model::{Meter, TICKS_PER_QUARTER, TempoMap, Tick};
use echode_realtime::Event;

use crate::clock::{sample_to_tick, tick_to_sample};
use crate::{ClockSource, InternalClock};

/// Why playback could not step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackError {
    InvalidSampleRate,
    InvalidLoop,
    InvalidFader,
    Time,
    BufferTooSmall,
    IdSpace,
    InvalidDevice,
    Plugin,
    /// The plugin loaded, but its audio ports are not a layout this host can run.
    Unsupported(String),
    Media,
}

impl std::fmt::Display for PlaybackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidSampleRate => "sample rate must be above zero",
            Self::InvalidLoop => "loop end must be after its start",
            Self::InvalidFader => "pan must be from -1 to 1 and gain must be finite",
            Self::Time => "playback position overflowed",
            Self::BufferTooSmall => "the caller buffer cannot hold this block",
            Self::IdSpace => "playback note numbers are exhausted",
            Self::InvalidDevice => "device state or parameter is not valid",
            Self::Plugin => "plugin helper did not start",
            Self::Unsupported(reason) => reason.as_str(),
            Self::Media => "audio file could not be read",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PlaybackError {}

/// One continuous run of samples inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayedSpan {
    pub start_sample: i64,
    pub frames: u32,
    /// Where this span starts in the block that was requested.
    pub block_offset: u32,
    /// Notes still sounding at this sample are released on the span's first frame.
    pub release_at: Option<i64>,
    /// Loop end in project samples, when a loop is set.
    pub loop_end: Option<i64>,
    /// Loop start after the playhead has wrapped. Absent before the first wrap.
    pub wrap_from: Option<i64>,
}

/// Transport for one sample rate and tempo map.
#[derive(Clone, Debug)]
pub struct Transport<C: ClockSource = InternalClock> {
    sample_rate: u32,
    tempo: TempoMap,
    clock: C,
    playing: bool,
    loop_start: Option<Tick>,
    loop_end: Option<Tick>,
    /// Set once the playhead has jumped back to the loop start.
    has_wrapped: bool,
    /// False after locating at/after the end; reaching the end from before wraps.
    loop_armed: bool,
}

impl Transport<InternalClock> {
    pub fn new(sample_rate: u32, tempo: TempoMap) -> Result<Self, PlaybackError> {
        Self::with_clock(sample_rate, tempo, InternalClock::default())
    }
}

impl<C: ClockSource> Transport<C> {
    pub fn with_clock(sample_rate: u32, tempo: TempoMap, clock: C) -> Result<Self, PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        Ok(Self {
            sample_rate,
            tempo,
            clock,
            playing: false,
            loop_start: None,
            loop_end: None,
            has_wrapped: false,
            loop_armed: false,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn position_sample(&self) -> i64 {
        self.clock.position()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    /// One transport event at `block_offset` for the project sample `sample`.
    pub fn transport_event(&self, sample: i64, block_offset: u32, meter: Meter) -> Event {
        let tick = sample_to_tick(sample, &self.tempo, self.sample_rate)
            .map(|tick| tick.get())
            .unwrap_or(0);
        let seconds = if self.sample_rate == 0 {
            0.0
        } else {
            sample as f64 / f64::from(self.sample_rate)
        };
        let (loop_start, loop_end) = match (self.loop_start, self.loop_end) {
            (Some(start), Some(end)) => (start.get(), end.get()),
            _ => (0, 0),
        };
        let bar = meter.bar_ticks().map(|tick| tick.get()).unwrap_or(1).max(1);
        let bar_index = tick.div_euclid(bar);
        Event::Transport(echode_realtime::Transport {
            time: block_offset,
            playing: self.playing,
            recording: false,
            loop_active: self.loop_start.is_some(),
            tempo_bpm: tempo_bpm(&self.tempo, tick),
            tempo_inc: 0.0,
            song_pos_beats: beats(tick),
            song_pos_seconds: seconds,
            loop_start_beats: beats(loop_start),
            loop_end_beats: beats(loop_end),
            loop_start_seconds: seconds_at(self, loop_start),
            loop_end_seconds: seconds_at(self, loop_end),
            bar_start: beats(bar_index.saturating_mul(bar)),
            bar_number: i32::try_from(bar_index).unwrap_or(i32::MAX),
            signature_numerator: u16::try_from(meter.numerator()).unwrap_or(u16::MAX),
            signature_denominator: u16::try_from(meter.denominator()).unwrap_or(u16::MAX),
        })
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    pub fn locate(&mut self, tick: Tick) -> Result<(), PlaybackError> {
        self.clock.locate(self.sample_at(tick)?);
        self.has_wrapped = false;
        self.arm_loop()?;
        Ok(())
    }

    pub fn set_tempo(&mut self, tempo: TempoMap) {
        self.tempo = tempo;
    }

    /// `end` is exclusive. `None` clears the loop.
    pub fn set_loop(&mut self, range: Option<(Tick, Tick)>) -> Result<(), PlaybackError> {
        if let Some((start, end)) = range {
            let start_sample = self.sample_at(start)?;
            let end_sample = self.sample_at(end)?;
            if end_sample <= start_sample {
                return Err(PlaybackError::InvalidLoop);
            }
            self.loop_start = Some(start);
            self.loop_end = Some(end);
        } else {
            self.loop_start = None;
            self.loop_end = None;
        }
        self.has_wrapped = false;
        self.arm_loop()?;
        Ok(())
    }

    /// Advances up to `frames` while playing and writes one span per continuous piece.
    ///
    /// The span buffer is scratch for this call, not a limit on loop length. On
    /// overflow the playhead is left where it was.
    pub fn pull(&mut self, frames: u32, spans: &mut [PlayedSpan]) -> Result<usize, PlaybackError> {
        if !self.playing || frames == 0 {
            return Ok(0);
        }
        let position = self.clock.position();
        let wrapped = self.has_wrapped;
        match self.pull_forward(frames, spans) {
            Ok(count) => Ok(count),
            Err(error) => {
                self.clock.locate(position);
                self.has_wrapped = wrapped;
                Err(error)
            }
        }
    }

    fn pull_forward(
        &mut self,
        frames: u32,
        spans: &mut [PlayedSpan],
    ) -> Result<usize, PlaybackError> {
        let mut budget = frames;
        let mut offset = 0u32;
        let mut count = 0;
        while budget > 0 {
            let Some(mut span) = self.next_span(&mut budget)? else {
                break;
            };
            span.block_offset = offset;
            offset = offset.checked_add(span.frames).ok_or(PlaybackError::Time)?;
            if count >= spans.len() {
                return Err(PlaybackError::BufferTooSmall);
            }
            spans[count] = span;
            count += 1;
        }
        Ok(count)
    }

    fn next_span(&mut self, budget: &mut u32) -> Result<Option<PlayedSpan>, PlaybackError> {
        if *budget == 0 {
            return Ok(None);
        }
        let mut release_at = None;
        if let Some((loop_start, loop_end)) = self.active_loop_samples()?
            && self.clock.position() >= loop_end
        {
            release_at = Some(loop_end);
            self.clock.locate(loop_start);
            self.has_wrapped = true;
        }
        let until_loop = self.frames_until_loop_end()?;
        let frames = (*budget).min(until_loop);
        if frames == 0 {
            return Err(PlaybackError::InvalidLoop);
        }
        let start = self.clock.position();
        let (loop_end, wrap_from) = match self.active_loop_samples()? {
            Some((loop_start, loop_end)) => {
                (Some(loop_end), self.has_wrapped.then_some(loop_start))
            }
            None => (None, None),
        };
        self.clock.advance(frames)?;
        *budget -= frames;
        Ok(Some(PlayedSpan {
            start_sample: start,
            frames,
            block_offset: 0,
            release_at,
            loop_end,
            wrap_from,
        }))
    }

    fn frames_until_loop_end(&self) -> Result<u32, PlaybackError> {
        let Some((_, loop_end)) = self.active_loop_samples()? else {
            return Ok(u32::MAX);
        };
        let room = loop_end.saturating_sub(self.clock.position());
        if room <= 0 {
            return Err(PlaybackError::InvalidLoop);
        }
        Ok(u32::try_from(room).unwrap_or(u32::MAX))
    }

    fn arm_loop(&mut self) -> Result<(), PlaybackError> {
        self.loop_armed = self
            .loop_samples()?
            .is_some_and(|(_, end)| self.clock.position() < end);
        Ok(())
    }

    fn active_loop_samples(&self) -> Result<Option<(i64, i64)>, PlaybackError> {
        if self.loop_armed {
            self.loop_samples()
        } else {
            Ok(None)
        }
    }

    fn loop_samples(&self) -> Result<Option<(i64, i64)>, PlaybackError> {
        let (Some(start), Some(end)) = (self.loop_start, self.loop_end) else {
            return Ok(None);
        };
        let start_sample = self.sample_at(start)?;
        let end_sample = self.sample_at(end)?;
        if end_sample <= start_sample {
            return Err(PlaybackError::InvalidLoop);
        }
        Ok(Some((start_sample, end_sample)))
    }

    fn sample_at(&self, tick: Tick) -> Result<i64, PlaybackError> {
        tick_to_sample(tick, &self.tempo, self.sample_rate).ok_or(PlaybackError::Time)
    }
}

fn beats(tick: i64) -> f64 {
    tick as f64 / TICKS_PER_QUARTER as f64
}

fn tempo_bpm(map: &TempoMap, tick: i64) -> f64 {
    let mut tempo = map.points()[0].tempo;
    for point in map.points() {
        if point.at.get() <= tick {
            tempo = point.tempo;
        } else {
            break;
        }
    }
    f64::from(tempo.numerator()) / f64::from(tempo.denominator())
}

fn seconds_at<C: ClockSource>(transport: &Transport<C>, tick: i64) -> f64 {
    let sample =
        tick_to_sample(Tick::new(tick), transport.tempo(), transport.sample_rate()).unwrap_or(0);
    if transport.sample_rate() == 0 {
        0.0
    } else {
        sample as f64 / f64::from(transport.sample_rate())
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
