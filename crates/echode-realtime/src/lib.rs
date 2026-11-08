//! Immutable playback plan and the cursors that walk it.
//!
//! Nothing here knows the document model. The control thread builds the plan
//! and the cursor buffers. Filling a block only advances indexes and rewrites
//! the live set in those buffers.

mod event;
pub mod plugin_capacity;

use std::sync::Arc;

pub use event::{
    ChokeEvent, Event, ExpressionEvent, ExpressionKind, NoteEvent, ParamEvent, Target, Transport,
    event_cmp, sort_events,
};

// Count inspected note payloads only in unit tests; release has no counter.
#[cfg(test)]
std::thread_local! {
    static NOTE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline(always)]
fn visit_note() {
    #[cfg(test)]
    NOTE_VISITS.with(|visits| visits.set(visits.get() + 1));
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
        self.cursor_with(indices)
            .expect("indices built from these tracks match them")
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
        let mut track_indices: Vec<_> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.track, index))
            .collect();
        track_indices.sort_unstable_by_key(|entry| entry.0);
        Some(Cursor {
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
            track_indices,
        })
    }
}

/// Read-only lookups a cursor derives from one track's notes.
///
/// Built on the control thread. Cursors of later plans share it while that
/// track's notes are unchanged, so an edit elsewhere does not sort them again.
#[derive(Debug, PartialEq, Eq)]
pub struct NoteIndex {
    notes: usize,
    /// Note model ID then onset. Repeated placements may share a model ID.
    by_id: Vec<u32>,
    ends: EndIndex,
}

impl NoteIndex {
    pub fn new(track: &TrackPlan) -> Self {
        let count = track.notes.len();
        let mut by_id: Vec<_> = (0..count as u32).collect();
        by_id.sort_unstable_by_key(|index| {
            let note = &track.notes[*index as usize];
            (note.note, note.on)
        });
        Self {
            notes: count,
            by_id,
            ends: EndIndex::new(&track.notes),
        }
    }
}

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
        for (index, note) in notes.iter().enumerate() {
            maxima[leaves + index] = note.off;
        }
        for index in (1..leaves).rev() {
            maxima[index] = maxima[index * 2].max(maxima[index * 2 + 1]);
        }
        Self { leaves, maxima }
    }
}

impl Cursor {
    fn track_slot(&self, id: u128) -> Option<usize> {
        self.track_indices
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()
            .map(|index| self.track_indices[index].1)
    }

    /// Chases notes already covering `sample` into the sounding set.
    ///
    /// Does not allocate and does not emit note-ons. The cursor buffers were
    /// allocated when the control thread built this cursor.
    pub fn catch_up(&mut self, plan: &Plan, sample: i64) {
        self.place(plan, sample, true);
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
            for (slot, index) in cursor.sounding[..cursor.sounding_len].iter().enumerate() {
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
    /// `self` must have been built from `plan` and not played.
    ///
    /// A track whose note index is shared with the same track of `previous`
    /// takes that track's state as is when `previous` stands at `sample`:
    /// nothing is searched, released or chased again, so the edit leaves its
    /// events as they would have been (R1). Every other track is placed at
    /// `sample` with a chase and keeps the key of each note `previous` still
    /// sounds, because a replaced plan may store a new pitch.
    ///
    /// `delivered` says the instruments heard `previous`'s sounding notes.
    /// Only then are the offs for the notes `plan` no longer covers at
    /// `sample` written to `out` (at offset 0), and the chased notes the
    /// previous plan covered are marked inherited for [`Self::chase_events`].
    /// Returns the number of offs. On an error neither cursor has changed.
    pub fn take_over(
        &mut self,
        plan: &Plan,
        previous: &mut Cursor,
        previous_plan: &Plan,
        sample: i64,
        delivered: bool,
        out: &mut [Event],
    ) -> Result<usize, PlayError> {
        let mut count = 0;
        if delivered {
            self.release_uncovered(plan, previous, previous_plan, sample, out, &mut count)?;
        }
        for slot in 0..self.tracks.len() {
            let Some(track) = plan.tracks.get(slot) else {
                continue;
            };
            let old = previous.track_slot(track.track);
            if let Some(old) = old.filter(|old| self.inherits(slot, previous, *old, sample)) {
                std::mem::swap(&mut self.tracks[slot], &mut previous.tracks[old]);
                let cursor = &mut self.tracks[slot];
                cursor.inherited = if delivered { cursor.sounding_len } else { 0 };
                continue;
            }
            let cursor = &mut self.tracks[slot];
            cursor.place(track, sample, true);
            let Some((old_cursor, old_track)) =
                old.and_then(|old| previous.tracks.get(old).zip(previous_plan.tracks.get(old)))
            else {
                continue;
            };
            for live in 0..cursor.sounding_len {
                visit_note();
                let index = cursor.sounding[live] as usize;
                let Some(note) = track.notes.get(index) else {
                    continue;
                };
                if let Some(old_index) = previous
                    .position
                    .and_then(|at| old_cursor.covering(old_track, note.note, at))
                    .filter(|old_index| old_cursor.sounding(*old_index))
                {
                    cursor.live_key[index] = old_cursor.live_key[old_index];
                }
                if delivered && old_cursor.covering(old_track, note.note, sample).is_some() {
                    cursor.inherit(live);
                }
            }
        }
        self.position = Some(sample);
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

    /// Note-offs for notes `previous` is sounding that `plan` does not cover
    /// at `sample`. A note that still covers that sample stays, including when
    /// only its pitch changed. A track `self` inherits as is releases nothing.
    /// `self`'s immutable indexes are ready before it is positioned.
    fn release_uncovered(
        &self,
        plan: &Plan,
        previous: &Cursor,
        previous_plan: &Plan,
        sample: i64,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        for (old, (cursor, track)) in previous
            .tracks
            .iter()
            .zip(&previous_plan.tracks)
            .enumerate()
        {
            let next = self.track_slot(track.track);
            if next.is_some_and(|slot| self.inherits(slot, previous, old, sample)) {
                continue;
            }
            for slot in 0..cursor.sounding_len {
                visit_note();
                let index = cursor.sounding[slot] as usize;
                let Some(note) = track.notes.get(index) else {
                    continue;
                };
                if next.is_some_and(|slot| {
                    self.tracks[slot]
                        .covering(&plan.tracks[slot], note.note, sample)
                        .is_some()
                }) {
                    continue;
                }
                let key = cursor.live_key.get(index).copied().unwrap_or(note.pitch);
                push_event(
                    out,
                    count,
                    Event::note_off(0, track.track_key, note.note_id, key),
                )?;
            }
        }
        Ok(())
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
                self.place(plan, span.start, false);
            } else if !continued {
                self.place(plan, span.start, true);
            }
            self.emit(plan, span, end, out, &mut count)?;
            self.position = Some(end);
        }
        if !sort_events(&mut out[..count], order, scratch) {
            return Err(PlayError::BufferTooSmall);
        }
        Ok(count)
    }

    fn place(&mut self, plan: &Plan, sample: i64, chase: bool) {
        for (cursor, track) in self.tracks.iter_mut().zip(&plan.tracks) {
            cursor.place(track, sample, chase);
        }
        self.position = Some(sample);
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
            for &index in &cursor.sounding[..cursor.sounding_len] {
                visit_note();
                let note = &track.notes[index as usize];
                let key = cursor.live_key[index as usize];
                push_event(
                    out,
                    count,
                    Event::note_off(block_offset, track.track_key, note.note_id, key),
                )?;
            }
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
    fn covering(&self, track: &TrackPlan, id: u128, sample: i64) -> Option<usize> {
        let end = self.index.by_id.partition_point(|index| {
            let note = &track.notes[*index as usize];
            (note.note, note.on) < (id, sample)
        });
        let index = *self.index.by_id.get(end.checked_sub(1)?)? as usize;
        visit_note();
        let note = track.notes.get(index)?;
        (note.note == id && note.on < sample && note.off >= sample).then_some(index)
    }

    fn place(&mut self, track: &TrackPlan, sample: i64, chase: bool) {
        self.inherited = 0;
        self.on_index = track.notes.partition_point(|note| note.on < sample);
        self.off_index = track
            .by_off
            .partition_point(|index| track.notes[*index as usize].off < sample);
        // Clear only the previous live set. No generation wrap can scan the score.
        while self.sounding_len != 0 {
            let index = self.sounding[self.sounding_len - 1] as usize;
            self.clear(index);
        }
        if !chase {
            return;
        }
        self.chase(track, sample, 1, 0, self.index.ends.leaves);
    }

    fn chase(&mut self, track: &TrackPlan, sample: i64, node: usize, start: usize, end: usize) {
        if start >= self.on_index
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
        self.chase(track, sample, node * 2, start, middle);
        self.chase(track, sample, node * 2 + 1, middle, end);
    }

    fn hold(&mut self, index: usize, key: u8) {
        if self.sounding(index) || self.sounding_len >= self.sounding.len() {
            return;
        }
        self.sounding[self.sounding_len] = index as u32;
        self.sounding_slots[index] = self.sounding_len;
        self.sounding_len += 1;
        self.live_key[index] = key;
    }

    /// Moves the live entry at `slot` to the end of the inherited prefix.
    fn inherit(&mut self, slot: usize) {
        let front = self.inherited;
        self.sounding.swap(slot, front);
        self.sounding_slots[self.sounding[slot] as usize] = slot;
        self.sounding_slots[self.sounding[front] as usize] = front;
        self.inherited += 1;
    }

    fn sounding(&self, index: usize) -> bool {
        self.sounding_slots
            .get(index)
            .is_some_and(|slot| *slot != usize::MAX)
    }

    fn clear(&mut self, index: usize) {
        if !self.sounding(index) {
            return;
        }
        let slot = self.sounding_slots[index];
        self.sounding_len -= 1;
        let last = self.sounding[self.sounding_len];
        self.sounding[slot] = last;
        self.sounding_slots[last as usize] = slot;
        self.sounding_slots[index] = usize::MAX;
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
            let on = track.notes.get(self.on_index).map(|note| note.on);
            let off = track
                .by_off
                .get(self.off_index)
                .map(|index| track.notes[*index as usize].off);
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
                let Some(off_at) = off else { break };
                if off_at >= end {
                    break;
                }
                let index = track.by_off[self.off_index] as usize;
                self.off_index += 1;
                if off_at >= span.start && self.sounding(index) {
                    let note = &track.notes[index];
                    let key = self.live_key.get(index).copied().unwrap_or(note.pitch);
                    push_event(
                        out,
                        count,
                        Event::note_off(offset(span, off_at)?, track.track_key, note.note_id, key),
                    )?;
                    self.clear(index);
                }
            } else {
                let Some(on_at) = on else { break };
                if on_at >= end {
                    break;
                }
                let index = self.on_index;
                self.on_index += 1;
                if on_at >= span.start {
                    let note = &track.notes[index];
                    push_event(
                        out,
                        count,
                        Event::note_on(
                            offset(span, on_at)?,
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
    if *count >= out.len() {
        return Err(PlayError::BufferTooSmall);
    }
    out[*count] = event;
    *count += 1;
    Ok(())
}

/// Indexes of clips that overlap the half-open sample range `[start, end)`.
///
/// `clips` is sorted by start and does not overlap. The range can cover more
/// than one clip when the block crosses a boundary.
pub fn overlapping_clips(clips: &[ClipSpan], start: i64, end: i64) -> std::ops::Range<usize> {
    let first = clips.partition_point(|clip| clip.end <= start);
    let mut last = first;
    while last < clips.len() && clips[last].start < end {
        last += 1;
    }
    first..last
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
