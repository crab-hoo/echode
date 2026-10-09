//! Immutable playback plan and the cursors that walk it.
//!
//! Nothing here knows the document model. The control thread builds the plan
//! and the cursor buffers. Filling a block only advances indexes and rewrites
//! marks in those buffers.

mod event;

pub use event::{
    ChokeEvent, Event, ExpressionEvent, ExpressionKind, NoteEvent, ParamEvent, Target, Transport,
    sort_events,
};

/// One tempo segment. `start_flick` is the rounded flick at `start_tick`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoSegment {
    pub start_tick: i64,
    pub start_flick: i128,
    pub numerator: u32,
    pub denominator: u32,
}

/// Tempo boundaries with cumulative flicks, so a conversion is one binary search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoIndex {
    segments: Vec<TempoSegment>,
}

impl TempoIndex {
    pub fn new(segments: Vec<TempoSegment>) -> Option<Self> {
        if segments.is_empty() {
            return None;
        }
        Some(Self { segments })
    }

    pub fn tick_to_sample(&self, tick: i64, sample_rate: u32) -> Option<i64> {
        if sample_rate == 0 {
            return None;
        }
        let flicks = self.tick_to_flick(tick)?;
        let samples = div_round(flicks * i128::from(sample_rate), FLICKS_PER_SECOND)?;
        i64::try_from(samples).ok()
    }

    pub fn sample_to_tick(&self, sample: i64, sample_rate: u32) -> Option<i64> {
        if sample_rate == 0 {
            return None;
        }
        let flicks = div_round(
            i128::from(sample) * FLICKS_PER_SECOND,
            i128::from(sample_rate),
        )?;
        self.flick_to_tick(flicks)
    }

    fn tick_to_flick(&self, tick: i64) -> Option<i128> {
        let index = self
            .segments
            .partition_point(|segment| segment.start_tick <= tick)
            .saturating_sub(1);
        let segment = self.segments.get(index).unwrap_or(&self.segments[0]);
        let delta = i128::from(tick) - i128::from(segment.start_tick);
        segment.start_flick.checked_add(flicks_for_ticks(
            delta,
            segment.numerator,
            segment.denominator,
        )?)
    }

    fn flick_to_tick(&self, flick: i128) -> Option<i64> {
        let mut index = self
            .segments
            .partition_point(|segment| segment.start_flick <= flick)
            .saturating_sub(1);
        if flick < self.segments[0].start_flick {
            index = 0;
        }
        let segment = &self.segments[index];
        let delta = flick - segment.start_flick;
        let ticks = ticks_for_flicks(delta, segment.numerator, segment.denominator)?;
        i64::try_from(i128::from(segment.start_tick).checked_add(ticks)?).ok()
    }
}

const FLICKS_PER_SECOND: i128 = 705_600_000;
const FLICKS_PER_TICK_AT_ONE_BPM: i128 = 60_000;

fn flicks_for_ticks(ticks: i128, numerator: u32, denominator: u32) -> Option<i128> {
    let scale = FLICKS_PER_TICK_AT_ONE_BPM.checked_mul(i128::from(denominator))?;
    div_round(ticks.checked_mul(scale)?, i128::from(numerator))
}

fn ticks_for_flicks(flicks: i128, numerator: u32, denominator: u32) -> Option<i128> {
    let scale = FLICKS_PER_TICK_AT_ONE_BPM.checked_mul(i128::from(denominator))?;
    div_round(flicks.checked_mul(i128::from(numerator))?, scale)
}

fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    if denominator <= 0 {
        return None;
    }
    let half = denominator / 2;
    let adjusted = if numerator >= 0 {
        numerator.checked_add(half)?
    } else {
        numerator.checked_sub(half)?
    };
    Some(adjusted / denominator)
}

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

/// Which mixer value a live update writes. The document model is not involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerKey {
    Gain,
    Pan,
    Mute,
    Solo,
}

/// One mixer write. It is `Copy` so the audio thread can take it without a heap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerChange {
    pub track: u128,
    pub key: MixerKey,
    pub value: f32,
}

/// One track's notes. `notes` is sorted by start. `by_off` indexes `notes` by end.
///
/// The mixer fields are the values at the moment the control thread built the
/// plan. A later [`MixerChange`] overwrites the field on the plan the audio
/// thread is holding.
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
    /// Writes one mixer value into the track the control thread named.
    /// Unknown tracks are left alone.
    pub fn write_mixer(&mut self, track: u128, key: MixerKey, value: f32) -> bool {
        let Some(slot) = self.tracks.iter_mut().find(|slot| slot.track == track) else {
            return false;
        };
        match key {
            MixerKey::Gain => slot.gain = value,
            MixerKey::Pan => slot.pan = value,
            MixerKey::Mute => slot.mute = value,
            MixerKey::Solo => slot.solo = value,
        }
        true
    }
}

impl Plan {
    pub fn cursor(&self) -> Cursor {
        Cursor {
            tracks: self
                .tracks
                .iter()
                .map(|track| {
                    let count = track.notes.len();
                    TrackCursor {
                        on_index: 0,
                        off_index: 0,
                        sounding: vec![0; count],
                        sounding_len: 0,
                        live_key: vec![0; count],
                        mark: vec![0; count],
                        epoch: 0,
                    }
                })
                .collect(),
            position: None,
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
    position: Option<i64>,
}

#[derive(Clone, Debug)]
struct TrackCursor {
    on_index: usize,
    off_index: usize,
    sounding: Vec<u32>,
    sounding_len: usize,
    /// Key sent with the note-on, indexed like `notes`. A later plan may change
    /// pitch; the note-off still uses this key.
    live_key: Vec<u8>,
    mark: Vec<u32>,
    epoch: u32,
}

impl Cursor {
    /// Chases notes already covering `sample` into the sounding set.
    ///
    /// Does not allocate and does not emit note-ons. The cursor buffers were
    /// allocated when the control thread built this cursor.
    pub fn catch_up(&mut self, plan: &Plan, sample: i64) {
        self.place(plan, sample, true);
    }

    /// Copies note-on keys from `previous` onto notes that are still sounding.
    ///
    /// A replaced plan may store a new pitch. The live voice was started with
    /// the old key, and its note-off has to use that key.
    pub fn keep_live_keys(&mut self, plan: &Plan, previous: &Cursor, previous_plan: &Plan) {
        for (cursor, track) in self.tracks.iter_mut().zip(&plan.tracks) {
            for slot in 0..cursor.sounding_len {
                let index = cursor.sounding[slot] as usize;
                let Some(note) = track.notes.get(index) else {
                    continue;
                };
                if let Some(key) = live_key_of(previous, previous_plan, track.track, note.note)
                    && index < cursor.live_key.len()
                {
                    cursor.live_key[index] = key;
                }
            }
        }
    }

    /// Note-offs for notes this cursor is sounding that `next` does not cover
    /// at `sample`. A note that still covers that sample stays, including when
    /// only its pitch changed. Offs are written at `block_offset`.
    pub fn release_replaced(
        &self,
        plan: &Plan,
        next: &Plan,
        sample: i64,
        block_offset: u32,
        out: &mut [Event],
    ) -> Result<usize, PlayError> {
        let mut count = 0;
        for (cursor, track) in self.tracks.iter().zip(&plan.tracks) {
            for slot in 0..cursor.sounding_len {
                let index = cursor.sounding[slot] as usize;
                let Some(note) = track.notes.get(index) else {
                    continue;
                };
                if covers(next, track.track, note.note, sample) {
                    continue;
                }
                let key = cursor.live_key.get(index).copied().unwrap_or(note.pitch);
                push_event(
                    out,
                    &mut count,
                    Event::note_off(block_offset, track.track_key, note.note_id, key),
                )?;
            }
        }
        Ok(count)
    }

    /// Plays `spans` into `out` and returns how many edges were written.
    ///
    /// A backward jump with `release_at` stops the notes that are sounding,
    /// then repositions without chasing. Any other landing chases notes that
    /// are already in progress so a later note-off still has a set entry.
    pub fn play(
        &mut self,
        plan: &Plan,
        spans: &[Span],
        out: &mut [Event],
    ) -> Result<usize, PlayError> {
        let mut count = 0;
        for span in spans {
            let end = span
                .start
                .checked_add(i64::from(span.frames))
                .ok_or(PlayError::Time)?;
            let continued = self.position == Some(span.start);
            if let Some(release) = span.release_at {
                self.release_covering(plan, span, release, out, &mut count)?;
                self.place(plan, span.start, false);
            } else if !continued {
                self.place(plan, span.start, true);
            }
            self.emit(plan, span, end, out, &mut count)?;
            self.position = Some(end);
        }
        sort_events(&mut out[..count]);
        Ok(count)
    }

    fn place(&mut self, plan: &Plan, sample: i64, chase: bool) {
        for (cursor, track) in self.tracks.iter_mut().zip(&plan.tracks) {
            cursor.place(track, sample, chase);
        }
        self.position = Some(sample);
    }

    fn release_covering(
        &mut self,
        plan: &Plan,
        span: &Span,
        release: i64,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
        for (cursor, track) in self.tracks.iter().zip(&plan.tracks) {
            let end = track.notes.partition_point(|note| note.on < release);
            for (index, note) in track.notes[..end].iter().enumerate() {
                if note.off >= release {
                    let key = if cursor.sounding(index) {
                        cursor.live_key.get(index).copied().unwrap_or(note.pitch)
                    } else {
                        note.pitch
                    };
                    push_event(
                        out,
                        count,
                        Event::note_off(span.block_offset, track.track_key, note.note_id, key),
                    )?;
                }
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
    fn place(&mut self, track: &TrackPlan, sample: i64, chase: bool) {
        self.on_index = track.notes.partition_point(|note| note.on < sample);
        self.off_index = track
            .by_off
            .partition_point(|index| track.notes[*index as usize].off < sample);
        self.sounding_len = 0;
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.mark.fill(0);
            self.epoch = 1;
        }
        if !chase {
            return;
        }
        for index in 0..self.on_index {
            if track.notes[index].off >= sample {
                let key = track.notes[index].pitch;
                self.hold(index, key);
            }
        }
    }

    fn hold(&mut self, index: usize, key: u8) {
        if self.sounding_len >= self.sounding.len() {
            return;
        }
        self.sounding[self.sounding_len] = index as u32;
        self.sounding_len += 1;
        self.mark[index] = self.epoch;
        if index < self.live_key.len() {
            self.live_key[index] = key;
        }
    }

    fn sounding(&self, index: usize) -> bool {
        self.mark.get(index).copied() == Some(self.epoch)
    }

    fn clear(&mut self, index: usize) {
        if index < self.mark.len() {
            self.mark[index] = 0;
        }
    }

    fn emit(
        &mut self,
        track: &TrackPlan,
        span: &Span,
        end: i64,
        out: &mut [Event],
        count: &mut usize,
    ) -> Result<(), PlayError> {
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

fn covers(plan: &Plan, track: u128, note: u128, sample: i64) -> bool {
    let Some(track) = plan.tracks.iter().find(|slot| slot.track == track) else {
        return false;
    };
    track
        .notes
        .iter()
        .any(|span| span.note == note && span.on < sample && span.off >= sample)
}

fn offset(span: &Span, sample: i64) -> Result<u32, PlayError> {
    let local = u32::try_from(sample - span.start).map_err(|_| PlayError::Time)?;
    span.block_offset.checked_add(local).ok_or(PlayError::Time)
}

fn live_key_of(cursor: &Cursor, plan: &Plan, track: u128, note: u128) -> Option<u8> {
    let (cursor, track) = cursor
        .tracks
        .iter()
        .zip(&plan.tracks)
        .find(|(_, slot)| slot.track == track)?;
    let index = cursor.sounding[..cursor.sounding_len]
        .iter()
        .map(|slot| *slot as usize)
        .find(|index| {
            track
                .notes
                .get(*index)
                .is_some_and(|span| span.note == note)
        })?;
    cursor.live_key.get(index).copied()
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
        cursor.play(&first, &[opened], &mut events).unwrap();
        assert_eq!(events[0].key(), Some(60));

        let next = plan(64);
        let mut swapped = next.cursor();
        swapped.catch_up(&next, 100);
        swapped.keep_live_keys(&next, &cursor, &first);
        let held = Span {
            start: 100,
            frames: 1_000,
            block_offset: 0,
            release_at: None,
        };
        let count = swapped.play(&next, &[held], &mut events).unwrap();
        assert_eq!(count, 1);
        assert!(events[0].is_note_off());
        assert_eq!(events[0].key(), Some(60));
        assert_eq!(events[0].note_id(), Some(3));
    }
}
