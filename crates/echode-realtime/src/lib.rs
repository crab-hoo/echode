//! Immutable playback plan and the cursors that walk it.
//!
//! Nothing here knows the document model. The control thread builds the plan
//! and the cursor buffers. Filling a block only advances indexes and rewrites
//! the live set in those buffers.

// The audio thread runs this crate. Product code reaches data with `get` and
// iterators, so a bad index is a skipped note, not a panic (ADR 0284).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

mod event;
pub mod plugin_capacity;

use std::sync::Arc;

pub use event::{
    ChokeEvent, Event, ExpressionEvent, ExpressionKind, NoteEvent, ParamEvent, Target, Transport,
    event_cmp, sort_events,
};

// Count inspected note payloads and searched placements only in unit tests;
// release has no counter.
#[cfg(test)]
std::thread_local! {
    static NOTE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PLACEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline(always)]
fn visit_note() {
    #[cfg(test)]
    NOTE_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[inline(always)]
fn search_placement() {
    #[cfg(test)]
    PLACEMENTS.with(|placements| placements.set(placements.get() + 1));
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod cursor_tests;

pub use echode_time::{TempoIndex, TempoSegment};

/// A note already clipped to its placement, in sample time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteSpan {
    pub on: i64,
    pub off: i64,
    pub note: u128,
    /// Stable for the life of one playback. Not the model id.
    pub note_id: u32,
    pub pitch: u8,
    /// Stored velocity, 0..=65535. 65535 is full scale.
    pub velocity: u16,
}

/// One track's notes. `notes` is sorted by start. `by_off` indexes `notes` by end.
/// Mixer values are the snapshot the control thread used to build this plan.
/// Transient previews live in the product playback bank, not in this score.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackPlan {
    pub track: u128,
    /// Stable for the life of one playback. Not the model id.
    pub track_key: u32,
    pub notes: Vec<NoteSpan>,
    pub by_off: Vec<u32>,
    pub gain: f32,
    pub pan: f32,
    pub mute: f32,
    pub solo: f32,
}

/// Half-open audio clip on the project timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipSpan {
    pub start: i64,
    pub end: i64,
}

/// Notes and clips for every track. Built off the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub tracks: Vec<TrackPlan>,
}

impl Plan {
    pub fn cursor(&self) -> Cursor {
        let indices = self
            .tracks
            .iter()
            .map(|track| Arc::new(NoteIndex::new(track)))
            .collect();
        self.cursor_over(indices)
    }

    /// A cursor that reads `indices[i]` for track `i`. Each index must have
    /// been built from notes equal to that track's, which a control thread can
    /// ensure by reusing it only with an unchanged track plan. `None` when the
    /// count or a note count does not match.
    ///
    /// [`Cursor::take_over`] relies on the same rule: a track whose index is
    /// shared with the previous cursor's same track has equal notes.
    pub fn cursor_with(&self, indices: Vec<Arc<NoteIndex>>) -> Option<Cursor> {
        if indices.len() != self.tracks.len()
            || self
                .tracks
                .iter()
                .zip(&indices)
                .any(|(track, index)| index.notes != track.notes.len())
        {
            return None;
        }
        Some(self.cursor_over(indices))
    }

    /// The cursor for `indices`, which already match these tracks.
    fn cursor_over(&self, indices: Vec<Arc<NoteIndex>>) -> Cursor {
        let mut track_indices: Vec<_> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.track, index))
            .collect();
        track_indices.sort_unstable_by_key(|entry| entry.0);
        Cursor {
            tracks: indices
                .into_iter()
                .map(|index| {
                    let count = index.notes;
                    TrackCursor {
                        on_index: 0,
                        off_index: 0,
                        sounding: vec![0; count],
                        sounding_len: 0,
                        inherited: 0,
                        live_key: vec![0; count],
                        sounding_slots: vec![usize::MAX; count],
                        index,
                    }
                })
                .collect(),
            position: None,
            chase_from: None,
            track_indices,
        }
    }
}

/// Read-only lookups a cursor derives from one track's notes.
///
/// Built on the control thread. Cursors of later plans share it while that
/// track's notes are unchanged, so an edit elsewhere does not sort them again.
#[derive(Debug, PartialEq, Eq)]
pub struct NoteIndex {
    notes: usize,
    ends: EndIndex,
}

impl NoteIndex {
    pub fn new(track: &TrackPlan) -> Self {
        Self {
            notes: track.notes.len(),
            ends: EndIndex::new(&track.notes),
        }
    }
}

/// Edges a pre-placed track may advance over when a plan is adopted. Past
/// this a search is cheaper, so the adoption block stays bounded either way.
const ADVANCE_EDGES: usize = 32;

/// One continuous piece of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: i64,
    pub frames: u32,
    pub block_offset: u32,
    pub release_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayError {
    Time,
    BufferTooSmall,
}

/// Indexes and the sounding set. The buffers are allocated with the cursor.
#[derive(Clone, Debug)]
pub struct Cursor {
    tracks: Vec<TrackCursor>,
    track_indices: Vec<(u128, usize)>,
    position: Option<i64>,
    /// Where the current stretch of playback chases from. `None` chased every
    /// note covering its landing; `Some(landing)` is a loop restart, which
    /// does not chase notes from before the loop (ADR 0239).
    chase_from: Option<i64>,
}

#[derive(Clone, Debug)]
struct TrackCursor {
    on_index: usize,
    off_index: usize,
    sounding: Vec<u32>,
    sounding_len: usize,
    /// `sounding[..inherited]` are notes the previous plan's instruments
    /// already hold. Set by [`Cursor::take_over`]; cleared when the set is
    /// placed again or a block is played.
    inherited: usize,
    /// Key sent with the note-on, indexed like `notes`. A later plan may change
    /// pitch; the note-off still uses this key.
    live_key: Vec<u8>,
    /// Reverse index into the dense live set; a stopped note has no slot.
    sounding_slots: Vec<usize>,
    /// Shared with other cursors. Retired cursors return to the control
    /// thread, so the audio thread never drops the last reference.
    index: Arc<NoteIndex>,
}

/// Built on the control thread. A seek visits overlapping notes, rather than
/// every note that has ever started before the playhead.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EndIndex {
    leaves: usize,
    maxima: Vec<i64>,
}

impl EndIndex {
    fn new(notes: &[NoteSpan]) -> Self {
        let leaves = notes.len().max(1).next_power_of_two();
        let mut maxima = vec![i64::MIN; leaves * 2];
        for (slot, note) in maxima.iter_mut().skip(leaves).zip(notes) {
            *slot = note.off;
        }
        // Node `n` holds the latest end of its children `2n` and `2n + 1`.
        for index in (1..leaves).rev() {
            let Some((parents, children)) = maxima.split_at_mut_checked(index * 2) else {
                continue;
            };
            if let (Some(node), Some([left, right])) =
                (parents.get_mut(index), children.first_chunk::<2>())
            {
                *node = (*left).max(*right);
            }
        }
        Self { leaves, maxima }
    }
}

impl TrackPlan {
    /// The end of `notes[index]`, where `index` is a `by_off` entry.
    fn off_at(&self, index: u32) -> Option<i64> {
        self.notes.get(index as usize).map(|note| note.off)
    }
}

impl Cursor {
    fn track_slot(&self, id: u128) -> Option<usize> {
        let index = self
            .track_indices
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()?;
        self.track_indices.get(index).map(|entry| entry.1)
    }

    /// Chases notes already covering `sample` into the sounding set.
    ///
    /// Does not allocate and does not emit note-ons. The cursor buffers were
    /// allocated when the control thread built this cursor.
    pub fn catch_up(&mut self, plan: &Plan, sample: i64) {
        self.place(plan, sample, None);
    }

    /// Like [`Self::catch_up`], but chases only notes that start at `from` or
    /// later when it is `Some`, as playback does after a loop restart.
    ///
    /// The control thread places a cursor this way at the last published
    /// position and [`Self::chase_from`], so [`Self::take_over`] only advances
    /// it over the notes since. Any other sample is correct, only slower.
    pub fn catch_up_from(&mut self, plan: &Plan, sample: i64, from: Option<i64>) {
        self.place(plan, sample, from);
    }

    /// Where the current stretch of playback chases from (see
    /// [`Self::catch_up_from`]).
    pub fn chase_from(&self) -> Option<i64> {
        self.chase_from
    }

    /// Note-ons for the chased set, for an instrument starting in mid-note.
    ///
    /// Notes the previous plan's instruments already hold (see
    /// [`Self::take_over`]) go to `inherited`, so only a newly started
    /// instrument strikes them; the others go to `fresh`. Returns both counts.
    ///
    /// A note that ends exactly at the landing stays in the set so its off
    /// still leaves with the next block, but it is not struck: that note-on
    /// would sort after its own off at the same sample and hold the note.
    pub fn chase_events(
        &self,
        plan: &Plan,
        fresh: &mut [Event],
        inherited: &mut [Event],
    ) -> Result<(usize, usize), PlayError> {
        let (mut struck, mut kept) = (0, 0);
        for (cursor, track) in self.tracks.iter().zip(&plan.tracks) {
            for (slot, index) in cursor.live().iter().enumerate() {
                visit_note();
                let Some(note) = track.notes.get(*index as usize) else {
                    continue;
                };
                if self.position.is_some_and(|landing| note.off <= landing) {
                    continue;
                }
                let event = Event::note_on(
                    0,
                    track.track_key,
                    note.note_id,
                    note.pitch,
                    unit_velocity(note.velocity),
                );
                if slot < cursor.inherited {
                    push_event(inherited, &mut kept, event)?;
                } else {
                    push_event(fresh, &mut struck, event)?;
                }
            }
        }
        Ok((struck, kept))
    }

    /// Takes over from `previous`, the cursor of `previous_plan`, at `sample`.
    /// `self` must have been built from `plan` and not played; the control
    /// thread may have placed it with [`Self::catch_up_from`].
    ///
    /// A track whose note index is shared with the same track of `previous`
    /// takes that track's state as is when `previous` stands at `sample`:
    /// nothing is searched, released or chased again, so the edit leaves its
    /// events as they would have been. Every other track stands at `sample`,
    /// chased by `previous`'s rule, so a loop restart still does not chase
    /// notes from before the loop. A track placed in advance by the same rule
    /// only advances over the notes since, when there are few.
    ///
    /// The voices of a replaced track are matched to `previous`'s by model
    /// note ID. A matched note keeps the key it started with, because the new
    /// plan may store a new pitch. When `delivered` (the instruments heard
    /// `previous`'s sounding notes), each unmatched voice of `previous` is
    /// released into `out` at offset 0, and each matched one is marked
    /// inherited for [`Self::chase_events`]. Returns the number of offs.
    ///
    /// On an error `previous` keeps its voices and `self` must be discarded.
    pub fn take_over(
        &mut self,
        plan: &Plan,
        previous: &mut Cursor,
        previous_plan: &Plan,
        sample: i64,
        delivered: bool,
        out: &mut [Event],
    ) -> Result<usize, PlayError> {
        let from = previous.chase_from;
        let placed = self.position.filter(|at| {
            self.chase_from == from && from.is_none_or(|from| from <= *at) && *at <= sample
        });
        for slot in 0..self.tracks.len() {
            let Some(track) = plan.tracks.get(slot) else {
                continue;
            };
            if previous
                .track_slot(track.track)
                .is_some_and(|old| self.inherits(slot, previous, old, sample))
            {
                continue;
            }
            let Some(cursor) = self.tracks.get_mut(slot) else {
                continue;
            };
            if placed.is_none() || !cursor.advance(track, sample) {
                cursor.place(track, sample, from);
            }
            cursor.sort_live(track);
        }
        let mut count = 0;
        for old in 0..previous.tracks.len() {
            let Some(old_track) = previous_plan.tracks.get(old) else {
                continue;
            };
            let next = self.track_slot(old_track.track);
            if next.is_some_and(|slot| self.inherits(slot, previous, old, sample)) {
                continue;
            }
            let Some(old_cursor) = previous.tracks.get_mut(old) else {
                continue;
            };
            old_cursor.sort_live(old_track);
            let old_cursor = &*old_cursor;
            match next.and_then(|slot| self.tracks.get_mut(slot).zip(plan.tracks.get(slot))) {
                Some((cursor, track)) => {
                    cursor.match_voices(track, old_cursor, old_track, delivered, out, &mut count)?
                }
                None if delivered => {
                    old_cursor.release_live(old_track, 0, out, &mut count)?;
                }
                None => {}
            }
        }
        // Nothing below can fail, so a failed release leaves `previous` whole.
        for slot in 0..self.tracks.len() {
            let Some(old) = plan
                .tracks
                .get(slot)
                .and_then(|track| previous.track_slot(track.track))
                .filter(|old| self.inherits(slot, previous, *old, sample))
            else {
                continue;
            };
            let (Some(cursor), Some(old_cursor)) =
                (self.tracks.get_mut(slot), previous.tracks.get_mut(old))
            else {
                continue;
            };
            std::mem::swap(cursor, old_cursor);
            cursor.inherited = if delivered { cursor.sounding_len } else { 0 };
        }
        self.position = Some(sample);
        self.chase_from = from;
        Ok(count)
    }

    /// Track `slot` of `self` takes track `old` of `previous` as is. The
    /// shared index means equal notes (see [`Plan::cursor_with`]).
    fn inherits(&self, slot: usize, previous: &Cursor, old: usize, sample: i64) -> bool {
        previous.position == Some(sample)
            && self
                .tracks
                .get(slot)
                .zip(previous.tracks.get(old))
                .is_some_and(|(next, old)| Arc::ptr_eq(&next.index, &old.index))
    }

    /// Plays `spans` into `out` and returns how many edges were written.
    ///
    /// A backward jump with `release_at` stops the notes that are sounding,
    /// then repositions without chasing. Any other landing chases notes that
    /// are already in progress so a later note-off still has a set entry.
    /// `order` and `scratch` sort the written edges and must be at least as
    /// long as the events this call writes.
    pub fn play(
        &mut self,
        plan: &Plan,
        spans: &[Span],
        out: &mut [Event],
        order: &mut [usize],
        scratch: &mut [Event],
    ) -> Result<usize, PlayError> {
        let mut count = 0;
        for span in spans {
            let end = span
                .start
                .checked_add(i64::from(span.frames))
                .ok_or(PlayError::Time)?;
            let continued = self.position == Some(span.start);
            if span.release_at.is_some() {
                self.release_all_into(plan, span.block_offset, out, &mut count)?;
                self.place(plan, span.start, Some(span.start));
            } else if !continued {
                self.place(plan, span.start, None);
            }
            self.emit(plan, span, end, out, &mut count)?;
            self.position = Some(end);
        }
        // `push_event` keeps `count` within `out`.
        let written = out.get_mut(..count).ok_or(PlayError::BufferTooSmall)?;
        if !sort_events(written, order, scratch) {
            return Err(PlayError::BufferTooSmall);
        }
        Ok(count)
    }

    fn place(&mut self, plan: &Plan, sample: i64, from: Option<i64>) {
        for (cursor, track) in self.tracks.iter_mut().zip(&plan.tracks) {
            cursor.place(track, sample, from);
        }
        self.position = Some(sample);
        self.chase_from = from;
    }

    /// Stops only live voices, without searching the score or testing a sentinel
    /// sample. The caller repositions or retires this cursor after the release.
    pub fn release_all(
        &self,
        plan: &Plan,
        block_offset: u32,
        out: &mut [Event],
    ) -> Result<usize, PlayError> {
        let mut count = 0;
        self.release_all_into(plan, block_offset, out, &mut count)?;
        Ok(count)
    }

    fn release_all_into(
        &self,
        plan: &Plan,
        block_offset: u32,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        for (cursor, track) in self.tracks.iter().zip(&plan.tracks) {
            cursor.release_live(track, block_offset, out, count)?;
        }
        Ok(())
    }

    fn emit(
        &mut self,
        plan: &Plan,
        span: &Span,
        end: i64,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        for (cursor, track) in self.tracks.iter_mut().zip(&plan.tracks) {
            cursor.emit(track, span, end, out, count)?;
        }
        Ok(())
    }
}

impl TrackCursor {
    /// The note indexes of the live voices.
    fn live(&self) -> &[u32] {
        self.sounding.get(..self.sounding_len).unwrap_or_default()
    }

    /// Note-offs for every live voice, with the keys they started with.
    fn release_live(
        &self,
        track: &TrackPlan,
        block_offset: u32,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        for &index in self.live() {
            visit_note();
            let Some(note) = track.notes.get(index as usize) else {
                continue;
            };
            push_event(
                out,
                count,
                Event::note_off(
                    block_offset,
                    track.track_key,
                    note.note_id,
                    self.key_of(index as usize, note),
                ),
            )?;
        }
        Ok(())
    }

    /// The key the note at `index` started with.
    fn key_of(&self, index: usize, note: &NoteSpan) -> u8 {
        self.live_key.get(index).copied().unwrap_or(note.pitch)
    }

    fn place(&mut self, track: &TrackPlan, sample: i64, from: Option<i64>) {
        search_placement();
        self.inherited = 0;
        self.on_index = track.notes.partition_point(|note| note.on < sample);
        self.off_index = track
            .by_off
            .partition_point(|index| track.off_at(*index).is_some_and(|off| off < sample));
        // Clear only the previous live set. No generation wrap can scan the score.
        let Self {
            sounding,
            sounding_len,
            sounding_slots,
            ..
        } = self;
        for &index in sounding.get(..*sounding_len).unwrap_or_default() {
            if let Some(slot) = sounding_slots.get_mut(index as usize) {
                *slot = usize::MAX;
            }
        }
        *sounding_len = 0;
        let first = match from {
            None => 0,
            Some(from) if from >= sample => return,
            Some(from) => track.notes.partition_point(|note| note.on < from),
        };
        self.chase(track, sample, first, 1, 0, self.index.ends.leaves);
    }

    /// Holds the notes in `[first, on_index)` of `[start, end)` that cover `sample`.
    fn chase(
        &mut self,
        track: &TrackPlan,
        sample: i64,
        first: usize,
        node: usize,
        start: usize,
        end: usize,
    ) {
        if start >= self.on_index
            || end <= first
            || self
                .index
                .ends
                .maxima
                .get(node)
                .is_none_or(|max| *max < sample)
        {
            return;
        }
        if end - start == 1 {
            visit_note();
            if let Some(note) = track.notes.get(start) {
                self.hold(start, note.pitch);
            }
            return;
        }
        let middle = start + (end - start) / 2;
        self.chase(track, sample, first, node * 2, start, middle);
        self.chase(track, sample, first, node * 2 + 1, middle, end);
    }

    /// Moves a placed track forward to `end` as a placement with the same
    /// chase rule would leave it. `false`, unchanged, when more than
    /// [`ADVANCE_EDGES`] onsets or ends lie before `end`.
    fn advance(&mut self, track: &TrackPlan, end: i64) -> bool {
        let onsets = track
            .notes
            .get(self.on_index + ADVANCE_EDGES)
            .is_some_and(|note| note.on < end);
        let ends = track
            .by_off
            .get(self.off_index + ADVANCE_EDGES)
            .is_some_and(|index| track.off_at(*index).is_some_and(|off| off < end));
        if onsets || ends {
            return false;
        }
        self.inherited = 0;
        while let Some(note) = track.notes.get(self.on_index).filter(|note| note.on < end) {
            visit_note();
            self.hold(self.on_index, note.pitch);
            self.on_index += 1;
        }
        while let Some(index) = track
            .by_off
            .get(self.off_index)
            .filter(|index| track.off_at(**index).is_some_and(|off| off < end))
        {
            visit_note();
            self.clear(*index as usize);
            self.off_index += 1;
        }
        true
    }

    /// Orders the live set by model note ID for [`Self::match_voices`].
    fn sort_live(&mut self, track: &TrackPlan) {
        let Some(live) = self.sounding.get_mut(..self.sounding_len) else {
            return;
        };
        live.sort_unstable_by_key(|index| {
            visit_note();
            track.notes.get(*index as usize).map(|note| note.note)
        });
        for (slot, index) in live.iter().enumerate() {
            if let Some(entry) = self.sounding_slots.get_mut(*index as usize) {
                *entry = slot;
            }
        }
    }

    /// Pairs this track's live set with `previous`'s, both sorted by model
    /// note ID, one voice to one voice. See [`Cursor::take_over`].
    fn match_voices(
        &mut self,
        track: &TrackPlan,
        previous: &TrackCursor,
        previous_track: &TrackPlan,
        delivered: bool,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        let old_live = previous.live();
        let old_note = |slot: usize| {
            let index = *old_live.get(slot)? as usize;
            visit_note();
            previous_track.notes.get(index).map(|note| (index, note))
        };
        let release = |(index, note): (usize, &NoteSpan), out: &mut [Event], count: &mut usize| {
            push_event(
                out,
                count,
                Event::note_off(
                    0,
                    previous_track.track_key,
                    note.note_id,
                    previous.key_of(index, note),
                ),
            )
        };
        let mut next_old = 0;
        // Entries before `live` are done. Inheriting swaps one of them, so the
        // rest stays sorted.
        for live in 0..self.sounding_len {
            let Some(index) = self.sounding.get(live).map(|index| *index as usize) else {
                break;
            };
            visit_note();
            let Some(id) = track.notes.get(index).map(|note| note.note) else {
                continue;
            };
            while let Some(old) = old_note(next_old)
                && old.1.note < id
            {
                if delivered {
                    release(old, out, count)?;
                }
                next_old += 1;
            }
            if let Some((old, note)) = old_note(next_old).filter(|(_, note)| note.note == id) {
                let key = previous.key_of(old, note);
                if let Some(entry) = self.live_key.get_mut(index) {
                    *entry = key;
                }
                next_old += 1;
                if delivered {
                    self.inherit(live);
                }
            }
        }
        if delivered {
            while let Some(old) = old_note(next_old) {
                release(old, out, count)?;
                next_old += 1;
            }
        }
        Ok(())
    }

    fn hold(&mut self, index: usize, key: u8) {
        if self.sounding(index) {
            return;
        }
        let (Some(entry), Some(slot), Some(live_key)) = (
            self.sounding.get_mut(self.sounding_len),
            self.sounding_slots.get_mut(index),
            self.live_key.get_mut(index),
        ) else {
            return;
        };
        *entry = index as u32;
        *slot = self.sounding_len;
        *live_key = key;
        self.sounding_len += 1;
    }

    /// Moves the live entry at `slot` to the end of the inherited prefix.
    fn inherit(&mut self, slot: usize) {
        let front = self.inherited;
        if front > slot || slot >= self.sounding_len {
            return;
        }
        self.sounding.swap(slot, front);
        for at in [slot, front] {
            if let Some(entry) = self
                .sounding
                .get(at)
                .and_then(|index| self.sounding_slots.get_mut(*index as usize))
            {
                *entry = at;
            }
        }
        self.inherited += 1;
    }

    fn sounding(&self, index: usize) -> bool {
        self.sounding_slots
            .get(index)
            .is_some_and(|slot| *slot != usize::MAX)
    }

    fn clear(&mut self, index: usize) {
        let Some(slot) = self
            .sounding_slots
            .get(index)
            .copied()
            .filter(|slot| *slot != usize::MAX)
        else {
            return;
        };
        let Some(last_slot) = self.sounding_len.checked_sub(1) else {
            return;
        };
        let Some(&last) = self.sounding.get(last_slot) else {
            return;
        };
        if let Some(entry) = self.sounding.get_mut(slot) {
            *entry = last;
        }
        if let Some(entry) = self.sounding_slots.get_mut(last as usize) {
            *entry = slot;
        }
        if let Some(entry) = self.sounding_slots.get_mut(index) {
            *entry = usize::MAX;
        }
        self.sounding_len = last_slot;
    }

    fn emit(
        &mut self,
        track: &TrackPlan,
        span: &Span,
        end: i64,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        // Clearing reorders the live set, so the inherited prefix ends here.
        self.inherited = 0;
        loop {
            let next_on = track.notes.get(self.on_index);
            let next_off = track
                .by_off
                .get(self.off_index)
                .and_then(|index| Some((*index as usize, track.notes.get(*index as usize)?)));
            let on = next_on.map(|note| note.on);
            let off = next_off.map(|(_, note)| note.off);
            let take_off = match (on, off) {
                (None, None) => break,
                (Some(on), _) if on >= end => match off {
                    Some(off) if off < end => true,
                    _ => break,
                },
                (_, Some(off)) if off >= end && on.is_none() => break,
                (Some(on), Some(off)) => off <= on,
                (None, Some(_)) => true,
                (Some(_), None) => false,
            };
            if take_off {
                let Some((index, note)) = next_off else { break };
                if note.off >= end {
                    break;
                }
                self.off_index += 1;
                if note.off >= span.start && self.sounding(index) {
                    push_event(
                        out,
                        count,
                        Event::note_off(
                            offset(span, note.off)?,
                            track.track_key,
                            note.note_id,
                            self.key_of(index, note),
                        ),
                    )?;
                    self.clear(index);
                }
            } else {
                let Some(note) = next_on else { break };
                if note.on >= end {
                    break;
                }
                let index = self.on_index;
                self.on_index += 1;
                if note.on >= span.start {
                    push_event(
                        out,
                        count,
                        Event::note_on(
                            offset(span, note.on)?,
                            track.track_key,
                            note.note_id,
                            note.pitch,
                            unit_velocity(note.velocity),
                        ),
                    )?;
                    self.hold(index, note.pitch);
                }
            }
        }
        Ok(())
    }
}

fn offset(span: &Span, sample: i64) -> Result<u32, PlayError> {
    let local = u32::try_from(sample - span.start).map_err(|_| PlayError::Time)?;
    span.block_offset.checked_add(local).ok_or(PlayError::Time)
}

fn unit_velocity(stored: u16) -> f64 {
    f64::from(stored) / 65535.0
}

fn push_event(out: &mut [Event], count: &mut usize, event: Event) -> Result<(), PlayError> {
    let slot = out.get_mut(*count).ok_or(PlayError::BufferTooSmall)?;
    *slot = event;
    *count += 1;
    Ok(())
}

/// Indexes of clips that overlap the half-open sample range `[start, end)`.
///
/// `clips` is sorted by start and does not overlap. The range can cover more
/// than one clip when the block crosses a boundary.
pub fn overlapping_clips(clips: &[ClipSpan], start: i64, end: i64) -> std::ops::Range<usize> {
    let first = clips.partition_point(|clip| clip.end <= start);
    let overlapping = clips
        .iter()
        .skip(first)
        .take_while(|clip| clip.start < end)
        .count();
    first..first + overlapping
}

#[cfg(test)]
mod key_tests {
    use super::{Event, NoteSpan, Plan, Span, TrackPlan};

    fn plan(pitch: u8) -> Plan {
        Plan {
            tracks: vec![TrackPlan {
                track: 1,
                track_key: 0,
                notes: vec![NoteSpan {
                    on: 0,
                    off: 1_000,
                    note: 7,
                    note_id: 3,
                    pitch,
                    velocity: 65_535,
                }],
                by_off: vec![0],
                gain: 1.0,
                pan: 0.0,
                mute: 0.0,
                solo: 0.0,
            }],
        }
    }

    #[test]
    fn a_replaced_note_offs_with_the_key_it_started_on() {
        let first = plan(60);
        let mut cursor = first.cursor();
        let mut events = [Event::note_off(0, 0, 0, 0); 4];
        let opened = Span {
            start: 0,
            frames: 64,
            block_offset: 0,
            release_at: None,
        };
        let mut order = [0usize; 4];
        let mut scratch = [Event::note_off(0, 0, 0, 0); 4];
        cursor
            .play(&first, &[opened], &mut events, &mut order, &mut scratch)
            .unwrap();
        assert_eq!(events[0].key(), Some(60));

        let next = plan(64);
        let mut swapped = next.cursor();
        let offs = swapped
            .take_over(&next, &mut cursor, &first, 100, true, &mut events)
            .unwrap();
        assert_eq!(offs, 0, "the replaced note still covers the landing");
        let held = Span {
            start: 100,
            frames: 1_000,
            block_offset: 0,
            release_at: None,
        };
        let count = swapped
            .play(&next, &[held], &mut events, &mut order, &mut scratch)
            .unwrap();
        assert_eq!(count, 1);
        assert!(events[0].is_note_off());
        assert_eq!(events[0].key(), Some(60));
        assert_eq!(events[0].note_id(), Some(3));
    }

    #[test]
    fn a_note_ending_at_the_landing_is_not_struck() {
        let plan = plan(60);
        let mut events = [Event::note_off(0, 0, 0, 0); 4];
        let mut order = [0usize; 4];
        let mut scratch = [Event::note_off(0, 0, 0, 0); 4];
        for (landing, struck) in [(999, 1), (1_000, 0)] {
            let mut cursor = plan.cursor();
            cursor.catch_up(&plan, landing);
            let (chased, _) = cursor.chase_events(&plan, &mut events, &mut []).unwrap();
            assert_eq!(chased, struck, "landing at {landing}");
            assert!(events[..chased].iter().all(|event| event.is_note_on()));
            // Either way the off still leaves at the note's end, after any
            // chased note-on, so nothing is left sounding.
            let span = Span {
                start: landing,
                frames: 64,
                block_offset: 0,
                release_at: None,
            };
            let count = cursor
                .play(&plan, &[span], &mut events, &mut order, &mut scratch)
                .unwrap();
            assert_eq!(count, 1);
            assert!(events[0].is_note_off());
            assert_eq!(i64::from(events[0].time()), 1_000 - landing);
        }
    }
}
