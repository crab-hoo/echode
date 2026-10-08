//! Play position in device samples.
//!
//! One call to [`Transport::pull`] moves the playhead and reports the sample
//! ranges the block covers. A loop produces at most one extra range, the piece
//! after the playhead jumps back. The caller supplies the range buffer.

use echode_model::{TempoMap, Tick};

use crate::clock::tick_to_sample;

/// Why playback could not step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackError {
    InvalidSampleRate,
    InvalidLoop,
    InvalidFader,
    Time,
    BufferTooSmall,
}

impl std::fmt::Display for PlaybackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidSampleRate => "sample rate must be above zero",
            Self::InvalidLoop => "loop end must be after its start",
            Self::InvalidFader => "pan must be from -1 to 1 and gain must be finite",
            Self::Time => "playback position overflowed",
            Self::BufferTooSmall => "the caller buffer cannot hold this block",
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
}

/// Transport for one sample rate and tempo map.
#[derive(Clone, Debug)]
pub struct Transport {
    sample_rate: u32,
    tempo: TempoMap,
    position: i64,
    playing: bool,
    loop_start: Option<Tick>,
    loop_end: Option<Tick>,
}

impl Transport {
    pub fn new(sample_rate: u32, tempo: TempoMap) -> Result<Self, PlaybackError> {
        if sample_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        Ok(Self {
            sample_rate,
            tempo,
            position: 0,
            playing: false,
            loop_start: None,
            loop_end: None,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn position_sample(&self) -> i64 {
        self.position
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    pub fn locate(&mut self, tick: Tick) -> Result<(), PlaybackError> {
        self.position = self.sample_at(tick)?;
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
        let position = self.position;
        match self.pull_forward(frames, spans) {
            Ok(count) => Ok(count),
            Err(error) => {
                self.position = position;
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
        if let Some((loop_start, loop_end)) = self.loop_samples()?
            && self.position >= loop_end
        {
            release_at = Some(loop_end);
            self.position = loop_start;
        }
        let until_loop = self.frames_until_loop_end()?;
        let frames = (*budget).min(until_loop);
        if frames == 0 {
            return Err(PlaybackError::InvalidLoop);
        }
        let start = self.position;
        self.position = self
            .position
            .checked_add(i64::from(frames))
            .ok_or(PlaybackError::Time)?;
        *budget -= frames;
        Ok(Some(PlayedSpan {
            start_sample: start,
            frames,
            block_offset: 0,
            release_at,
        }))
    }

    fn frames_until_loop_end(&self) -> Result<u32, PlaybackError> {
        let Some((_, loop_end)) = self.loop_samples()? else {
            return Ok(u32::MAX);
        };
        let room = loop_end.saturating_sub(self.position);
        if room <= 0 {
            return Err(PlaybackError::InvalidLoop);
        }
        Ok(u32::try_from(room).unwrap_or(u32::MAX))
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

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::Tempo;

    fn transport() -> Transport {
        Transport::new(
            48_000,
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn stop_holds_the_playhead() {
        let mut transport = transport();
        transport.locate(Tick::quarters(1).unwrap()).unwrap();
        let mut spans = [PlayedSpan {
            start_sample: 0,
            frames: 0,
            block_offset: 0,
            release_at: None,
        }; 4];
        assert_eq!(transport.pull(64, &mut spans).unwrap(), 0);
        assert_eq!(transport.position_sample(), 24_000);
        transport.play();
        assert_eq!(transport.pull(64, &mut spans).unwrap(), 1);
        assert_eq!(spans[0].start_sample, 24_000);
        assert_eq!(spans[0].frames, 64);
        assert_eq!(transport.position_sample(), 24_064);
    }

    #[test]
    fn a_loop_wraps_once_and_releases_notes_at_the_boundary() {
        let mut transport = transport();
        transport
            .set_loop(Some((Tick::new(0), Tick::quarters(1).unwrap())))
            .unwrap();
        transport.play();
        let mut spans = [PlayedSpan {
            start_sample: 0,
            frames: 0,
            block_offset: 0,
            release_at: None,
        }; 4];
        let count = transport.pull(24_064, &mut spans).unwrap();
        assert_eq!(count, 2);
        assert_eq!(spans[0].start_sample, 0);
        assert_eq!(spans[0].frames, 24_000);
        assert_eq!(spans[0].release_at, None);
        assert_eq!(spans[1].start_sample, 0);
        assert_eq!(spans[1].frames, 64);
        assert_eq!(spans[1].block_offset, 24_000);
        assert_eq!(spans[1].release_at, Some(24_000));
        assert_eq!(transport.position_sample(), 64);
    }

    #[test]
    fn a_loop_that_does_not_move_is_rejected() {
        let mut transport = transport();
        let error = transport
            .set_loop(Some((
                Tick::quarters(2).unwrap(),
                Tick::quarters(2).unwrap(),
            )))
            .unwrap_err();
        assert_eq!(error, PlaybackError::InvalidLoop);
        assert!(
            Transport::new(0, TempoMap::new(Tempo::quarters_per_minute(120).unwrap())).is_err()
        );
    }

    #[test]
    fn an_undersized_span_buffer_does_not_move_the_playhead() {
        let mut transport = transport();
        transport
            .set_loop(Some((Tick::new(0), Tick::quarters(1).unwrap())))
            .unwrap();
        transport.play();
        let mut spans = [PlayedSpan {
            start_sample: 0,
            frames: 0,
            block_offset: 0,
            release_at: None,
        }; 1];
        assert_eq!(
            transport.pull(24_064, &mut spans).unwrap_err(),
            PlaybackError::BufferTooSmall
        );
        assert_eq!(transport.position_sample(), 0);
    }
}
