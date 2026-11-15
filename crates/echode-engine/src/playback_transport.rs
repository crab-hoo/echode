//! Control-prepared musical time, shared by every track in one audio block.
use super::{PlaybackError, Playhead, RenderRange, split_loop};
use echode_model::{Meter, Project, TICKS_PER_QUARTER};
use echode_realtime::{Event, Span, TempoIndex, Transport};
use echode_time::{FLICKS_PER_SECOND, sample_to_flick};

struct Point {
    sample: i64,
    flick: i128,
    beats: f64,
    bpm: f64,
}

pub(crate) struct Timeline {
    points: Vec<Point>,
    rate: u32,
    meter: Meter,
    loop_bounds: Option<(i64, i64)>,
    loop_beats: (f64, f64),
    events: Vec<Event>,
    spans: Vec<Span>,
}

impl Timeline {
    pub(super) fn new(
        project: &Project,
        index: &TempoIndex,
        rate: u32,
        frames: usize,
        bounds: Option<(i64, i64)>,
    ) -> Result<Self, PlaybackError> {
        let mut points: Vec<Point> = Vec::new();
        for (position, point) in project.tempo().points().iter().enumerate() {
            let boundary_flick = index
                .tick_to_flick(point.at.get())
                .ok_or(PlaybackError::Time)?;
            // As in TempoIndex's negative intervals, anchor before zero at the
            // right boundary. This also avoids subtracting huge beat positions
            // when an old tempo point is far before a playhead near zero.
            let anchor = if point.at.get() < 0 {
                project
                    .tempo()
                    .points()
                    .get(position + 1)
                    .map(|next| next.at.get().min(0))
                    .unwrap_or(0)
            } else if position == 0 {
                0
            } else {
                point.at.get()
            };
            let flick = index.tick_to_flick(anchor).ok_or(PlaybackError::Time)?;
            let sample = match index.tick_to_sample(point.at.get(), rate) {
                Some(sample) => sample,
                // Unreachable future points cannot affect any i64 playback sample.
                // Before i64::MIN, retain the latest anchor for extrapolation.
                None if boundary_flick < 0 => i64::MIN,
                None => continue,
            };
            let compiled = Point {
                sample,
                flick,
                beats: anchor as f64 / TICKS_PER_QUARTER as f64,
                bpm: f64::from(point.tempo.numerator()) / f64::from(point.tempo.denominator()),
            };

            // Multiple changes rounded onto one sample have only one observable state.
            if points.last().is_some_and(|last| last.sample == sample) {
                points.pop();
            }
            points.push(compiled);
        }
        if points.is_empty() || frames == 0 || rate == 0 {
            return Err(PlaybackError::Time);
        }
        // Bound local density, then possible loop spans. Never reserve a score-sized
        // event list per track, nor impose a fixed tempo/loop/track limit.
        let mut first = 1;
        let mut changes = 0;
        for last in 1..points.len() {
            let at = points.get(last).ok_or(PlaybackError::Time)?.sample;
            while points
                .get(first)
                .is_some_and(|point| i128::from(at) - i128::from(point.sample) >= frames as i128)
            {
                first += 1;
            }
            changes = changes.max(last + 1 - first);
        }
        let spans = if let Some((start, end)) = bounds {
            let length = i128::from(end) - i128::from(start);
            if length <= 0 {
                return Err(PlaybackError::InvalidLoop);
            }
            ((frames as i128 / length) + 2).min(frames as i128) as usize
        } else {
            1
        };
        let capacity = changes
            .saturating_add(1)
            .saturating_mul(spans)
            .min(frames)
            .max(1);
        let mut timeline = Self {
            points,
            rate,
            meter: project.meter(),
            loop_bounds: bounds,
            loop_beats: (0.0, 0.0),
            events: Vec::with_capacity(capacity),
            spans: vec![
                Span {
                    start: 0,
                    frames: 0,
                    block_offset: 0,
                    release_at: None
                };
                spans
            ],
        };
        if let Some((start, end)) = bounds {
            timeline.loop_beats = (timeline.position(start)?.0, timeline.position(end)?.0);
        }
        Ok(timeline)
    }

    pub(super) fn capacity(&self) -> usize {
        self.events.capacity()
    }

    fn point_at(&self, sample: i64) -> usize {
        self.points
            .partition_point(|point| point.sample <= sample)
            .saturating_sub(1)
    }

    fn position(&self, sample: i64) -> Result<(f64, f64), PlaybackError> {
        let point = self
            .points
            .get(self.point_at(sample))
            .ok_or(PlaybackError::Time)?;
        let elapsed = sample_to_flick(sample, self.rate)
            .and_then(|flick| flick.checked_sub(point.flick))
            .ok_or(PlaybackError::Time)?;
        // Keep sub-tick precision. Integer tick rounding at each block start would
        // make a tempo-synced plugin depend on the callback partition.
        Ok((
            point.beats + elapsed as f64 * point.bpm / (60.0 * FLICKS_PER_SECOND as f64),
            point.bpm,
        ))
    }

    fn push(&mut self, sample: i64, time: u32, playing: bool) -> Result<(), PlaybackError> {
        let (beats, bpm) = self.position(sample)?;
        let bar_beats =
            f64::from(self.meter.numerator()) * 4.0 / f64::from(self.meter.denominator());
        let bar = (beats / bar_beats).floor();
        let (loop_start, loop_end) = self.loop_bounds.unwrap_or((0, 0));
        if self.events.len() == self.events.capacity() {
            return Err(PlaybackError::BufferTooSmall);
        }
        self.events.push(Event::Transport(Transport {
            time,
            playing,
            recording: false,
            loop_active: self.loop_bounds.is_some(),
            tempo_bpm: bpm,
            tempo_inc: 0.0,
            song_pos_beats: beats,
            song_pos_seconds: sample as f64 / f64::from(self.rate),
            loop_start_beats: self.loop_beats.0,
            loop_end_beats: self.loop_beats.1,
            loop_start_seconds: loop_start as f64 / f64::from(self.rate),
            loop_end_seconds: loop_end as f64 / f64::from(self.rate),
            bar_start: bar * bar_beats,
            bar_number: bar.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
            signature_numerator: u16::try_from(self.meter.numerator()).unwrap_or(u16::MAX),
            signature_denominator: u16::try_from(self.meter.denominator()).unwrap_or(u16::MAX),
        }));
        Ok(())
    }

    pub(super) fn stopped(&mut self, position: i64) -> Result<&[Event], PlaybackError> {
        self.events.clear();
        self.push(position, 0, false)?;
        Ok(&self.events)
    }
    pub(crate) fn delayed(
        &mut self,
        trajectory: Playhead,
        latency: u64,
        frames: usize,
    ) -> Result<&[Event], PlaybackError> {
        let (start, end) = trajectory.loop_bounds();
        let (count, _) = split_loop(
            trajectory.input_sample(latency)?,
            frames,
            start,
            end,
            &mut self.spans,
        )?;
        self.events.clear();
        for index in 0..count {
            let span = *self.spans.get(index).ok_or(PlaybackError::BufferTooSmall)?;
            self.push_span(span)?;
        }
        Ok(&self.events)
    }

    fn push_span(&mut self, span: Span) -> Result<(), PlaybackError> {
        if span.frames == 0 {
            return Ok(());
        }
        self.push(span.start, span.block_offset, true)?;
        let end = span
            .start
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        let mut next = self.point_at(span.start) + 1;
        while let Some(point) = self.points.get(next) {
            if point.sample >= end {
                break;
            }
            let at = point.sample;
            let offset = u32::try_from(at - span.start).map_err(|_| PlaybackError::Time)?;
            self.push(
                at,
                span.block_offset
                    .checked_add(offset)
                    .ok_or(PlaybackError::Time)?,
                true,
            )?;
            next += 1;
        }
        Ok(())
    }
    pub(super) fn fill(
        &mut self,
        spans: &[Span],
        range: RenderRange,
    ) -> Result<&[Event], PlaybackError> {
        self.events.clear();
        if let Some(held) = range.started_before {
            // A live stop's source fade is not continued musical playback.
            return self.stopped(held);
        } else {
            for span in spans {
                self.push_span(*span)?;
            }
        }
        Ok(&self.events)
    }
}

#[cfg(test)]
#[path = "playback_transport_tests.rs"]
mod tests;
