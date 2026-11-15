//! Per-stage note cursors over shared scores, using the audio graph's input lag.
use std::sync::Arc;

use echode_realtime::{Cursor, Event, NoteIndex, Plan, Span, TrackPlan, sort_events};

use crate::PlaybackError;
use crate::playback::Playhead;
use crate::playback::split_loop;

const EMPTY: Event = Event::note_off(0, 0, 0, 0);

/// One track's immutable score and lookups, shared across stages and unchanged plans.
#[derive(Clone, Debug)]
pub(crate) struct Score {
    plan: Arc<Plan>,
    pub(crate) index: Arc<NoteIndex>,
}

impl Score {
    pub(crate) fn new(track: &TrackPlan, index: Arc<NoteIndex>) -> Self {
        Self {
            plan: Arc::new(Plan {
                tracks: vec![track.clone()],
            }),
            index,
        }
    }
}

pub(crate) struct Notes {
    score: Score,
    cursor: Cursor,
    spans: Vec<Span>,
    events: Vec<Event>,
    chased: Vec<Event>,
    order: Vec<usize>,
    scratch: Vec<Event>,
    count: usize,
    chased_count: usize,
    pending: usize,
    active: bool,
    chase: bool,
    relocate: bool,
    next_sample: Option<i64>,
}

impl Notes {
    /// Called on control only. `capacity` includes local edges, chase and old releases.
    pub(crate) fn new(score: Score, frames: usize, capacity: usize) -> Result<Self, PlaybackError> {
        let cursor = score
            .plan
            .cursor_with(vec![Arc::clone(&score.index)])
            .ok_or(PlaybackError::Misaligned)?;
        Ok(Self {
            score,
            cursor,
            spans: vec![
                Span {
                    start: 0,
                    frames: 0,
                    block_offset: 0,
                    release_at: None
                };
                frames.checked_add(1).ok_or(PlaybackError::Time)?
            ],
            events: vec![EMPTY; capacity],
            chased: vec![EMPTY; capacity],
            order: vec![0; capacity],
            scratch: vec![EMPTY; capacity],
            count: 0,
            chased_count: 0,
            pending: 0,
            active: false,
            chase: false,
            relocate: false,
            next_sample: None,
        })
    }

    pub(crate) fn carry(
        &mut self,
        old: &mut Self,
        trajectory: Playhead,
        latency: u64,
    ) -> Result<(), PlaybackError> {
        if !old.active {
            return Ok(());
        }
        if trajectory.note_offset(latency, 1) != 0 {
            self.pending = old
                .cursor
                .release_all(&old.score.plan, 0, &mut self.events)
                .map_err(play_error)?;
            return Ok(());
        }
        self.pending = self
            .cursor
            .take_over(
                &self.score.plan,
                &mut old.cursor,
                &old.score.plan,
                trajectory.note_sample(latency)?,
                true,
                &mut self.events,
            )
            .map_err(play_error)?;
        self.active = true;
        self.next_sample = Some(trajectory.note_sample(latency)?);
        self.chase = true;
        Ok(())
    }

    pub(crate) fn prepare(
        &mut self,
        trajectory: Playhead,
        latency: u64,
        frames: usize,
        recover: bool,
    ) -> Result<(), PlaybackError> {
        if self.relocate {
            self.stop()?;
            self.pending = self.count;
        }
        self.count = self.pending;
        self.pending = 0;
        self.chased_count = 0;
        // Unlike transport's extrapolated clock, notes must not play before the
        // source trajectory has reached this stage (including a held-note start).
        let offset = trajectory.note_offset(latency, frames);
        if offset == frames {
            return Ok(());
        }
        let sample = if offset == 0 {
            trajectory.note_sample(latency)?
        } else {
            trajectory.note_origin()
        };
        if self.active && self.next_sample != Some(sample) {
            self.pending = self.count;
            self.stop()?;
        }
        let (start, end) = trajectory.loop_bounds();
        let (count, next_sample) =
            split_loop(sample, frames - offset, start, end, &mut self.spans)?;
        let spans = self
            .spans
            .get_mut(..count)
            .ok_or(PlaybackError::BufferTooSmall)?;
        for span in spans.iter_mut() {
            span.block_offset = span
                .block_offset
                .checked_add(offset as u32)
                .ok_or(PlaybackError::Time)?;
        }
        if !self.active {
            self.cursor.catch_up(&self.score.plan, sample);
            self.active = true;
            self.chase = true;
        }
        let chase = std::mem::take(&mut self.chase);
        if chase && spans.first().is_some_and(|span| span.release_at.is_none()) {
            let (fresh, inherited) = self
                .cursor
                .chase_events(
                    &self.score.plan,
                    self.events
                        .get_mut(self.count..)
                        .ok_or(PlaybackError::BufferTooSmall)?,
                    &mut self.chased,
                )
                .map_err(play_error)?;
            self.count += fresh;
            self.chased_count = inherited;
        } else if recover && spans.first().is_some_and(|span| span.release_at.is_none()) {
            self.chased_count = self
                .cursor
                .held_events(&self.score.plan, &mut self.chased, |_| true)
                .map_err(play_error)?;
        }
        // Chases begin where the source arrives, not necessarily at block zero.
        for event in self
            .events
            .get_mut(..self.count)
            .ok_or(PlaybackError::BufferTooSmall)?
        {
            if let Event::NoteOn(note) = event {
                note.time = offset as u32;
            }
        }
        for event in self
            .chased
            .get_mut(..self.chased_count)
            .ok_or(PlaybackError::BufferTooSmall)?
        {
            if let Event::NoteOn(note) = event {
                note.time = offset as u32;
            }
        }
        self.count += self
            .cursor
            .play(
                &self.score.plan,
                spans,
                self.events
                    .get_mut(self.count..)
                    .ok_or(PlaybackError::BufferTooSmall)?,
                &mut self.order,
                &mut self.scratch,
            )
            .map_err(play_error)?;
        self.next_sample = Some(next_sample);
        if !sort_events(
            self.events
                .get_mut(..self.count)
                .ok_or(PlaybackError::BufferTooSmall)?,
            &mut self.order,
            &mut self.scratch,
        ) || !sort_events(
            self.chased
                .get_mut(..self.chased_count)
                .ok_or(PlaybackError::BufferTooSmall)?,
            &mut self.order,
            &mut self.scratch,
        ) {
            return Err(PlaybackError::BufferTooSmall);
        }
        Ok(())
    }

    /// Stop immediately at a lifecycle boundary; never replay delayed future ons.
    pub(crate) fn stop(&mut self) -> Result<(), PlaybackError> {
        self.count = self.pending;
        self.pending = 0;
        self.chased_count = 0;
        if self.active {
            self.count += self
                .cursor
                .release_all(
                    &self.score.plan,
                    0,
                    self.events
                        .get_mut(self.count..)
                        .ok_or(PlaybackError::BufferTooSmall)?,
                )
                .map_err(play_error)?;
        }
        self.active = false;
        self.chase = false;
        self.relocate = false;
        Ok(())
    }

    pub(crate) fn reset(&mut self) {
        // Preserve live voices until their offs can leave in a discarded or
        // relocated block, even when the helper has already reset its DSP.
        self.relocate = true;
    }

    pub(crate) fn active(&self) -> bool {
        self.active || self.pending != 0
    }
    pub(crate) fn events(&self) -> Result<&[Event], PlaybackError> {
        self.events
            .get(..self.count)
            .ok_or(PlaybackError::BufferTooSmall)
    }
    pub(crate) fn chased(&self) -> Result<&[Event], PlaybackError> {
        self.chased
            .get(..self.chased_count)
            .ok_or(PlaybackError::BufferTooSmall)
    }
}

fn play_error(error: echode_realtime::PlayError) -> PlaybackError {
    match error {
        echode_realtime::PlayError::Time => PlaybackError::Time,
        echode_realtime::PlayError::BufferTooSmall => PlaybackError::BufferTooSmall,
    }
}

#[cfg(test)]
#[path = "plugin_notes_tests.rs"]
mod tests;
