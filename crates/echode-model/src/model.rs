//! Typeless tracks and smart clips.
//!
//! A track has no instrument-or-audio kind. An `auto` clip is the smallest run
//! of bars that contains its notes, and it grows into a neighboring bar when a
//! note is placed there. Editing the clip's bounds makes it `fixed`, after
//! which notes outside it start a new `auto` clip.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use crate::id::Id;
use crate::time::{Meter, Tempo, TempoMap, Tick};

/// Why a model edit was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    InvalidNote,
    InvalidClip,
    InvalidMeter,
    MissingTrack,
    MissingClip,
    MissingNote,
    DuplicateId,
    Overlap,
    Cycle,
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidNote => "note pitch, velocity, or length is not usable",
            Self::InvalidClip => "clip bounds are empty or out of range",
            Self::InvalidMeter => "meter cannot change while a track has clips",
            Self::MissingTrack => "track is not in the project",
            Self::MissingClip => "clip is not on the track",
            Self::MissingNote => "note is not on the track",
            Self::DuplicateId => "id is already used in the project",
            Self::Overlap => "clip would overlap another clip",
            Self::Cycle => "a track cannot contain itself",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ModelError {}

/// A note placed on the project timeline, before it belongs to a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteDraft {
    pub pitch: u8,
    pub velocity: u8,
    pub start: Tick,
    pub length: Tick,
}

/// A note stored relative to its clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: Id,
    pub start: Tick,
    pub length: Tick,
    pub pitch: u8,
    pub velocity: u8,
}

/// Whether a clip still follows its notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Auto,
    Fixed,
}

/// A time range that owns notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub id: Id,
    pub start: Tick,
    pub length: Tick,
    pub kind: ClipKind,
    pub notes: Vec<Note>,
}

impl Clip {
    /// Project time of a note, or `None` if the offset overflows.
    pub fn absolute_start(&self, note: &Note) -> Option<Tick> {
        self.start
            .get()
            .checked_add(note.start.get())
            .map(Tick::new)
    }
}

/// A track. MIDI and audio can share one; this slice stores notes only.
///
/// `parent` is another track, so folders can nest. There is no depth cap.
/// A folder is any track that others name as their parent. Summing children
/// onto that bus happens later; this only records the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: Id,
    pub name: String,
    pub parent: Option<Id>,
    pub clips: Vec<Clip>,
}

/// Immutable project snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    meter: Meter,
    tempo: TempoMap,
    tracks: Vec<Arc<Track>>,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    pub fn new() -> Self {
        let tempo = match Tempo::quarters_per_minute(120) {
            Some(tempo) => tempo,
            None => unreachable!("120 BPM is inside the allowed range"),
        };
        Self::with_signature(Meter::four_four(), tempo)
    }

    pub fn with_signature(meter: Meter, tempo: Tempo) -> Self {
        Self {
            meter,
            tempo: TempoMap::new(tempo),
            tracks: Vec::new(),
        }
    }

    pub fn meter(&self) -> Meter {
        self.meter
    }

    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn tracks(&self) -> &[Arc<Track>] {
        &self.tracks
    }

    /// Changes the bar grid. Notes are not refit, so this is only for an empty project.
    pub fn with_meter(&self, meter: Meter) -> Result<Self, ModelError> {
        if self.tracks.iter().any(|track| !track.clips.is_empty()) {
            return Err(ModelError::InvalidMeter);
        }
        Ok(Self {
            meter,
            tempo: self.tempo.clone(),
            tracks: self.tracks.clone(),
        })
    }

    #[must_use]
    pub fn with_tempo(&self, at: Tick, tempo: Tempo) -> Self {
        Self {
            meter: self.meter,
            tempo: self.tempo.with_tempo(at, tempo),
            tracks: self.tracks.clone(),
        }
    }

    #[must_use]
    pub fn add_track(&self, name: impl Into<String>) -> (Self, Id) {
        let name = name.into();
        let id = Id::generate();
        match self.add_track_with_id(id, name) {
            Ok(project) => (project, id),
            Err(error) => unreachable!("a generated track id is unique, but {error} was returned"),
        }
    }

    /// Adds a track with an id chosen by the caller, so a journal can replay it.
    pub fn add_track_with_id(&self, id: Id, name: impl Into<String>) -> Result<Self, ModelError> {
        if self.id_in_use(id) {
            return Err(ModelError::DuplicateId);
        }
        let mut tracks = self.tracks.clone();
        tracks.push(Arc::new(Track {
            id,
            name: name.into(),
            parent: None,
            clips: Vec::new(),
        }));
        Ok(Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            tracks,
        })
    }

    pub fn insert_note(&self, track_id: Id, draft: NoteDraft) -> Result<(Self, Id), ModelError> {
        let note_id = Id::generate();
        let clip_id = Id::generate();
        self.insert_note_with_id(track_id, note_id, clip_id, draft)
            .map(|project| (project, note_id))
    }

    /// Adds a track inside `parent`, after that folder's current subtree.
    pub fn add_child(&self, parent: Id, name: impl Into<String>) -> Result<(Self, Id), ModelError> {
        let id = Id::generate();
        self.add_child_with_id(id, parent, name)
            .map(|project| (project, id))
    }

    /// Adds a child with an id chosen by the caller, so a journal can replay it.
    pub fn add_child_with_id(
        &self,
        id: Id,
        parent: Id,
        name: impl Into<String>,
    ) -> Result<Self, ModelError> {
        if self.id_in_use(id) {
            return Err(ModelError::DuplicateId);
        }
        let parent_index = self.track_index(parent)?;
        let mut tracks = self.tracks.clone();
        let mut insert_at = parent_index + 1;
        while insert_at < tracks.len() && under(&tracks, parent, tracks[insert_at].id) {
            insert_at += 1;
        }
        tracks.insert(
            insert_at,
            Arc::new(Track {
                id,
                name: name.into(),
                parent: Some(parent),
                clips: Vec::new(),
            }),
        );
        Ok(Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            tracks,
        })
    }

    /// Moves a track under another folder, or to the top level when `parent` is `None`.
    ///
    /// Its children stay attached, so the subtree moves together. The arrange
    /// order does not change. A track cannot move into its own subtree.
    pub fn set_parent(&self, track_id: Id, parent: Option<Id>) -> Result<Self, ModelError> {
        let index = self.track_index(track_id)?;
        if let Some(parent_id) = parent {
            self.track_index(parent_id)?;
            if parent_chain_contains(&self.tracks, parent_id, track_id) {
                return Err(ModelError::Cycle);
            }
        }
        if self.tracks[index].parent == parent {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[index]).clone();
        track.parent = parent;
        Ok(self.write_track(index, track))
    }

    pub fn parent(&self, id: Id) -> Result<Option<Id>, ModelError> {
        let index = self.track_index(id)?;
        Ok(self.tracks[index].parent)
    }

    /// Direct children, in arrange order.
    pub fn children(&self, id: Id) -> Result<Vec<Id>, ModelError> {
        self.track_index(id)?;
        Ok(self
            .tracks
            .iter()
            .filter(|track| track.parent == Some(id))
            .map(|track| track.id)
            .collect())
    }

    /// Places a note using caller-chosen ids.
    ///
    /// `clip_id` is used only when this note creates a clip. A note that joins
    /// an existing clip keeps that clip's id.
    pub fn insert_note_with_id(
        &self,
        track_id: Id,
        note_id: Id,
        clip_id: Id,
        draft: NoteDraft,
    ) -> Result<Self, ModelError> {
        let index = self.track_index(track_id)?;
        if self.id_in_use(note_id) {
            return Err(ModelError::DuplicateId);
        }
        let clip_id_free = note_id != clip_id && !self.id_in_use(clip_id);
        let track = insert_note(
            &self.tracks[index],
            self.meter,
            note_id,
            clip_id,
            clip_id_free,
            draft,
        )?;
        Ok(self.write_track(index, track))
    }

    pub fn delete_note(&self, track_id: Id, note_id: Id) -> Result<Self, ModelError> {
        let index = self.track_index(track_id)?;
        let track = delete_note(&self.tracks[index], self.meter, note_id)?;
        Ok(self.write_track(index, track))
    }

    /// Moves a clip and marks it fixed. Note positions on the timeline stay put.
    pub fn resize_clip(
        &self,
        track_id: Id,
        clip_id: Id,
        start: Tick,
        length: Tick,
    ) -> Result<Self, ModelError> {
        let index = self.track_index(track_id)?;
        let track = resize_clip(&self.tracks[index], clip_id, start, length)?;
        Ok(self.write_track(index, track))
    }

    fn track_index(&self, id: Id) -> Result<usize, ModelError> {
        self.tracks
            .iter()
            .position(|track| track.id == id)
            .ok_or(ModelError::MissingTrack)
    }

    fn id_in_use(&self, id: Id) -> bool {
        self.tracks.iter().any(|track| {
            track.id == id
                || track
                    .clips
                    .iter()
                    .any(|clip| clip.id == id || clip.notes.iter().any(|note| note.id == id))
        })
    }

    /// Builds a project from saved parts.
    ///
    /// Ids must be unique. Auto clips are fitted to their notes, and an empty
    /// auto clip is dropped. Fixed clips keep the bounds they were given.
    pub fn try_assemble(
        meter: Meter,
        tempo: TempoMap,
        tracks: Vec<Track>,
    ) -> Result<Self, ModelError> {
        let bar = meter.bar_ticks().ok_or(ModelError::InvalidMeter)?.get();
        let mut seen = HashSet::new();
        let mut assembled = Vec::with_capacity(tracks.len());
        for mut track in tracks {
            if !seen.insert(track.id) {
                return Err(ModelError::DuplicateId);
            }
            let mut clips = Vec::with_capacity(track.clips.len());
            for mut clip in track.clips {
                if !seen.insert(clip.id) {
                    return Err(ModelError::DuplicateId);
                }
                if clip.length.get() <= 0
                    || clip.start.get().checked_add(clip.length.get()).is_none()
                {
                    return Err(ModelError::InvalidClip);
                }
                for note in &clip.notes {
                    if !seen.insert(note.id) {
                        return Err(ModelError::DuplicateId);
                    }
                    validate_note(&NoteDraft {
                        pitch: note.pitch,
                        velocity: note.velocity,
                        start: note.start,
                        length: note.length,
                    })?;
                }
                if clip.kind == ClipKind::Auto {
                    if clip.notes.is_empty() {
                        continue;
                    }
                    refit_auto(&mut clip, bar)?;
                }
                clips.push(clip);
            }
            sort_clips(&mut clips);
            if clips
                .iter()
                .enumerate()
                .any(|(index, _)| overlaps_others(&clips, index))
            {
                return Err(ModelError::Overlap);
            }
            track.clips = clips;
            assembled.push(Arc::new(track));
        }
        validate_forest(&assembled)?;
        Ok(Self {
            meter,
            tempo,
            tracks: assembled,
        })
    }

    fn write_track(&self, index: usize, track: Track) -> Self {
        let mut tracks = self.tracks.clone();
        tracks[index] = Arc::new(track);
        Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            tracks,
        }
    }

    /// Builds a snapshot without any checks, so a test can break a rule on purpose.
    #[cfg(test)]
    pub(crate) fn unchecked(meter: Meter, tempo: TempoMap, tracks: Vec<Track>) -> Self {
        Self {
            meter,
            tempo,
            tracks: tracks.into_iter().map(Arc::new).collect(),
        }
    }
}

fn under(tracks: &[Arc<Track>], folder: Id, candidate: Id) -> bool {
    let mut cursor = parent_of(tracks, candidate);
    let mut seen = HashSet::new();
    while let Some(id) = cursor {
        if id == folder {
            return true;
        }
        if !seen.insert(id) {
            return false;
        }
        cursor = parent_of(tracks, id);
    }
    false
}

fn parent_chain_contains(tracks: &[Arc<Track>], start: Id, target: Id) -> bool {
    let mut cursor = Some(start);
    let mut seen = HashSet::new();
    while let Some(id) = cursor {
        if id == target || !seen.insert(id) {
            return true;
        }
        cursor = parent_of(tracks, id);
    }
    false
}

fn parent_of(tracks: &[Arc<Track>], id: Id) -> Option<Id> {
    tracks
        .iter()
        .find(|track| track.id == id)
        .and_then(|track| track.parent)
}

fn validate_forest(tracks: &[Arc<Track>]) -> Result<(), ModelError> {
    let ids: HashSet<Id> = tracks.iter().map(|track| track.id).collect();
    for track in tracks {
        let Some(parent) = track.parent else {
            continue;
        };
        if !ids.contains(&parent) {
            return Err(ModelError::MissingTrack);
        }
        if parent_chain_contains(tracks, parent, track.id) {
            return Err(ModelError::Cycle);
        }
    }
    Ok(())
}

fn insert_note(
    track: &Track,
    meter: Meter,
    note_id: Id,
    clip_id: Id,
    clip_id_free: bool,
    draft: NoteDraft,
) -> Result<Track, ModelError> {
    validate_note(&draft)?;
    let bar = meter.bar_ticks().ok_or(ModelError::InvalidMeter)?.get();
    let mut track = track.clone();
    if let Some(index) = track
        .clips
        .iter()
        .position(|clip| contains_start(clip, draft.start.get()))
    {
        place_inside(&mut track.clips, index, note_id, &draft, bar)?;
    } else {
        place_outside(
            &mut track.clips,
            note_id,
            clip_id,
            clip_id_free,
            &draft,
            bar,
        )?;
    }
    sort_clips(&mut track.clips);
    Ok(track)
}

fn place_inside(
    clips: &mut [Clip],
    index: usize,
    note_id: Id,
    draft: &NoteDraft,
    bar: i64,
) -> Result<(), ModelError> {
    push_note(&mut clips[index], note_id, draft)?;
    if clips[index].kind == ClipKind::Auto {
        refit_auto(&mut clips[index], bar)?;
        if overlaps_others(clips, index) {
            return Err(ModelError::Overlap);
        }
    }
    Ok(())
}

fn place_outside(
    clips: &mut Vec<Clip>,
    note_id: Id,
    clip_id: Id,
    clip_id_free: bool,
    draft: &NoteDraft,
    bar: i64,
) -> Result<(), ModelError> {
    let note_bars =
        occupied_bars(draft.start.get(), draft.length.get(), bar).ok_or(ModelError::InvalidNote)?;
    let mut hosts = Vec::new();
    for (index, clip) in clips.iter().enumerate() {
        if clip.kind != ClipKind::Auto {
            continue;
        }
        let Some(clip_bars) = clip_bars(clip, bar) else {
            continue;
        };
        if bars_touch(clip_bars, note_bars) && !fixed_blocks(clips, clip_bars, note_bars, bar) {
            hosts.push(index);
        }
    }
    if hosts.is_empty() {
        return push_new_auto(clips, note_id, clip_id, clip_id_free, draft, note_bars, bar);
    }
    merge_autos(clips, &hosts, note_id, draft, bar)
}

fn push_new_auto(
    clips: &mut Vec<Clip>,
    note_id: Id,
    clip_id: Id,
    clip_id_free: bool,
    draft: &NoteDraft,
    note_bars: (i64, i64),
    bar: i64,
) -> Result<(), ModelError> {
    if !clip_id_free {
        return Err(ModelError::DuplicateId);
    }
    let start = note_bars
        .0
        .checked_mul(bar)
        .ok_or(ModelError::InvalidNote)?;
    let end = note_bars
        .1
        .checked_add(1)
        .and_then(|index| index.checked_mul(bar))
        .ok_or(ModelError::InvalidNote)?;
    let length = end.checked_sub(start).ok_or(ModelError::InvalidNote)?;
    if overlaps_span(clips, start, end) {
        return Err(ModelError::Overlap);
    }
    clips.push(Clip {
        id: clip_id,
        start: Tick::new(start),
        length: Tick::new(length),
        kind: ClipKind::Auto,
        notes: vec![Note {
            id: note_id,
            start: Tick::new(draft.start.get() - start),
            length: draft.length,
            pitch: draft.pitch,
            velocity: draft.velocity,
        }],
    });
    Ok(())
}

fn merge_autos(
    clips: &mut Vec<Clip>,
    hosts: &[usize],
    note_id: Id,
    draft: &NoteDraft,
    bar: i64,
) -> Result<(), ModelError> {
    let mut order: Vec<usize> = hosts.to_vec();
    order.sort_by(|&left, &right| {
        clips[left]
            .start
            .cmp(&clips[right].start)
            .then(clips[left].id.cmp(&clips[right].id))
    });
    let primary = clips[order[0]].id;
    let mut notes = Vec::new();
    for index in &order {
        absorb_notes(&clips[*index], &mut notes)?;
    }
    notes.push(Note {
        id: note_id,
        start: draft.start,
        length: draft.length,
        pitch: draft.pitch,
        velocity: draft.velocity,
    });
    let remove: Vec<Id> = order.iter().map(|&index| clips[index].id).collect();
    clips.retain(|clip| !remove.contains(&clip.id));
    let mut merged = Clip {
        id: primary,
        start: Tick::new(0),
        length: Tick::new(0),
        kind: ClipKind::Auto,
        notes,
    };
    refit_auto(&mut merged, bar)?;
    let end = merged
        .start
        .get()
        .checked_add(merged.length.get())
        .ok_or(ModelError::InvalidNote)?;
    if overlaps_span(clips, merged.start.get(), end) {
        return Err(ModelError::Overlap);
    }
    clips.push(merged);
    Ok(())
}

fn absorb_notes(clip: &Clip, into: &mut Vec<Note>) -> Result<(), ModelError> {
    for note in &clip.notes {
        let start = clip.absolute_start(note).ok_or(ModelError::InvalidNote)?;
        into.push(Note {
            start,
            ..note.clone()
        });
    }
    Ok(())
}

fn delete_note(track: &Track, meter: Meter, note_id: Id) -> Result<Track, ModelError> {
    let bar = meter.bar_ticks().ok_or(ModelError::InvalidMeter)?.get();
    let mut track = track.clone();
    let Some(index) = track
        .clips
        .iter()
        .position(|clip| clip.notes.iter().any(|note| note.id == note_id))
    else {
        return Err(ModelError::MissingNote);
    };
    track.clips[index].notes.retain(|note| note.id != note_id);
    match track.clips[index].kind {
        ClipKind::Auto if track.clips[index].notes.is_empty() => {
            track.clips.remove(index);
        }
        ClipKind::Auto => refit_auto(&mut track.clips[index], bar)?,
        ClipKind::Fixed => {}
    }
    Ok(track)
}

fn resize_clip(track: &Track, clip_id: Id, start: Tick, length: Tick) -> Result<Track, ModelError> {
    if length.get() <= 0 || start.get().checked_add(length.get()).is_none() {
        return Err(ModelError::InvalidClip);
    }
    let mut track = track.clone();
    let Some(index) = track.clips.iter().position(|clip| clip.id == clip_id) else {
        return Err(ModelError::MissingClip);
    };
    let delta = start
        .get()
        .checked_sub(track.clips[index].start.get())
        .ok_or(ModelError::InvalidClip)?;
    for note in &mut track.clips[index].notes {
        let relative = note
            .start
            .get()
            .checked_sub(delta)
            .ok_or(ModelError::InvalidNote)?;
        note.start = Tick::new(relative);
    }
    track.clips[index].start = start;
    track.clips[index].length = length;
    track.clips[index].kind = ClipKind::Fixed;
    if overlaps_others(&track.clips, index) {
        return Err(ModelError::Overlap);
    }
    sort_clips(&mut track.clips);
    Ok(track)
}

fn validate_note(draft: &NoteDraft) -> Result<(), ModelError> {
    if draft.pitch > 127 || draft.velocity > 127 || draft.length.get() <= 0 {
        return Err(ModelError::InvalidNote);
    }
    draft
        .start
        .get()
        .checked_add(draft.length.get())
        .ok_or(ModelError::InvalidNote)?;
    Ok(())
}

fn push_note(clip: &mut Clip, note_id: Id, draft: &NoteDraft) -> Result<(), ModelError> {
    let relative = draft
        .start
        .get()
        .checked_sub(clip.start.get())
        .ok_or(ModelError::InvalidNote)?;
    clip.notes.push(Note {
        id: note_id,
        start: Tick::new(relative),
        length: draft.length,
        pitch: draft.pitch,
        velocity: draft.velocity,
    });
    Ok(())
}

fn refit_auto(clip: &mut Clip, bar: i64) -> Result<(), ModelError> {
    if clip.notes.is_empty() {
        return Err(ModelError::InvalidNote);
    }
    let mut min_bar = i64::MAX;
    let mut max_bar = i64::MIN;
    let mut absolutes = Vec::with_capacity(clip.notes.len());
    for note in &clip.notes {
        let absolute = clip
            .absolute_start(note)
            .ok_or(ModelError::InvalidNote)?
            .get();
        let (start_bar, end_bar) =
            occupied_bars(absolute, note.length.get(), bar).ok_or(ModelError::InvalidNote)?;
        min_bar = min_bar.min(start_bar);
        max_bar = max_bar.max(end_bar);
        absolutes.push(absolute);
    }
    let new_start = min_bar.checked_mul(bar).ok_or(ModelError::InvalidNote)?;
    let new_end = max_bar
        .checked_add(1)
        .and_then(|index| index.checked_mul(bar))
        .ok_or(ModelError::InvalidNote)?;
    for (note, absolute) in clip.notes.iter_mut().zip(absolutes) {
        note.start = Tick::new(
            absolute
                .checked_sub(new_start)
                .ok_or(ModelError::InvalidNote)?,
        );
    }
    clip.start = Tick::new(new_start);
    clip.length = Tick::new(
        new_end
            .checked_sub(new_start)
            .ok_or(ModelError::InvalidNote)?,
    );
    Ok(())
}

fn contains_start(clip: &Clip, at: i64) -> bool {
    match span(clip) {
        Some((start, end)) => at >= start && at < end,
        None => false,
    }
}

fn span(clip: &Clip) -> Option<(i64, i64)> {
    clip.start
        .get()
        .checked_add(clip.length.get())
        .map(|end| (clip.start.get(), end))
}

fn overlaps_others(clips: &[Clip], index: usize) -> bool {
    let Some(range) = span(&clips[index]) else {
        return true;
    };
    clips.iter().enumerate().any(|(other_index, other)| {
        other_index != index
            && span(other).is_some_and(|other_range| spans_overlap(range, other_range))
    })
}

fn overlaps_span(clips: &[Clip], start: i64, end: i64) -> bool {
    clips
        .iter()
        .any(|clip| span(clip).is_some_and(|range| spans_overlap(range, (start, end))))
}

fn spans_overlap(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

fn clip_bars(clip: &Clip, bar: i64) -> Option<(i64, i64)> {
    occupied_bars(clip.start.get(), clip.length.get(), bar)
}

fn occupied_bars(start: i64, length: i64, bar: i64) -> Option<(i64, i64)> {
    if length <= 0 || bar <= 0 {
        return None;
    }
    let last = start.checked_add(length - 1)?;
    Some((start.div_euclid(bar), last.div_euclid(bar)))
}

fn bars_touch(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 <= right.1.saturating_add(1) && right.0 <= left.1.saturating_add(1)
}

fn fixed_blocks(clips: &[Clip], left: (i64, i64), right: (i64, i64), bar: i64) -> bool {
    let low = left.0.min(right.0);
    let high = left.1.max(right.1);
    clips.iter().any(|clip| {
        clip.kind == ClipKind::Fixed
            && clip_bars(clip, bar).is_some_and(|bars| bars.0 <= high && low <= bars.1)
    })
}

fn sort_clips(clips: &mut [Clip]) {
    clips.sort_by(|left, right| left.start.cmp(&right.start).then(left.id.cmp(&right.id)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::time::{Meter, TICKS_PER_QUARTER, Tempo, TempoMap, Tick};
    use std::sync::Arc;

    fn at(start_quarters: i64, length_quarters: i64) -> NoteDraft {
        NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::quarters(start_quarters).unwrap(),
            length: Tick::quarters(length_quarters).unwrap(),
        }
    }

    fn project_with_track() -> (Project, Id) {
        Project::new().add_track("piano")
    }

    fn only_clip(project: &Project) -> &Clip {
        assert_eq!(project.tracks()[0].clips.len(), 1);
        &project.tracks()[0].clips[0]
    }

    #[test]
    fn placing_a_note_creates_one_bar_auto_clip() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(1, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.kind, ClipKind::Auto);
        assert_eq!(clip.start.get(), 0);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(
            clip.absolute_start(&clip.notes[0]).unwrap(),
            Tick::quarters(1).unwrap()
        );
    }

    #[test]
    fn the_next_bar_extends_an_auto_clip_without_moving_notes() {
        let (project, track) = project_with_track();
        let (project, first) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 8);
        let kept = clip.notes.iter().find(|note| note.id == first).unwrap();
        assert_eq!(clip.absolute_start(kept).unwrap().get(), 0);
    }

    #[test]
    fn a_gap_starts_a_second_clip_until_the_gap_is_filled() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
        assert_eq!(project.tracks()[0].clips.len(), 2);
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.notes.len(), 3);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 12);
    }

    #[test]
    fn deleting_notes_shrinks_and_then_removes_an_auto_clip() {
        let (project, track) = project_with_track();
        let (project, early) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, late) = project.insert_note(track, at(4, 1)).unwrap();
        let project = project.delete_note(track, late).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(clip.absolute_start(&clip.notes[0]).unwrap().get(), 0);
        let project = project.delete_note(track, early).unwrap();
        assert!(project.tracks()[0].clips.is_empty());
    }

    #[test]
    fn resizing_fixes_the_bounds_and_a_neighbor_starts_a_new_clip() {
        let (project, track) = project_with_track();
        let (project, note) = project.insert_note(track, at(0, 1)).unwrap();
        let clip_id = project.tracks()[0].clips[0].id;
        let project = project
            .resize_clip(track, clip_id, Tick::new(0), Tick::quarters(4).unwrap())
            .unwrap();
        assert_eq!(only_clip(&project).kind, ClipKind::Fixed);
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        assert_eq!(project.tracks()[0].clips.len(), 2);
        assert_eq!(project.tracks()[0].clips[0].kind, ClipKind::Fixed);
        assert_eq!(project.tracks()[0].clips[1].kind, ClipKind::Auto);
        let fixed = &project.tracks()[0].clips[0];
        assert_eq!(fixed.absolute_start(&fixed.notes[0]).unwrap().get(), 0);
        assert_eq!(fixed.notes[0].id, note);
    }

    #[test]
    fn a_note_inside_a_fixed_clip_does_not_resize_it() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let clip_id = project.tracks()[0].clips[0].id;
        let project = project
            .resize_clip(track, clip_id, Tick::new(0), Tick::quarters(8).unwrap())
            .unwrap();
        let (project, _) = project.insert_note(track, at(5, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.kind, ClipKind::Fixed);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 8);
        assert_eq!(clip.notes.len(), 2);
    }

    #[test]
    fn an_auto_clip_stops_at_a_following_fixed_clip() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
        let later = project.tracks()[0].clips[1].id;
        let project = project
            .resize_clip(
                track,
                later,
                Tick::quarters(8).unwrap(),
                Tick::quarters(4).unwrap(),
            )
            .unwrap();
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        assert_eq!(project.tracks()[0].clips.len(), 2);
        assert_eq!(project.tracks()[0].clips[0].kind, ClipKind::Auto);
        assert_eq!(
            project.tracks()[0].clips[0].length.get(),
            TICKS_PER_QUARTER * 8
        );
        assert_eq!(project.tracks()[0].clips[1].kind, ClipKind::Fixed);
        assert_eq!(
            project.tracks()[0].clips[1].start,
            Tick::quarters(8).unwrap()
        );
    }

    #[test]
    fn a_note_that_runs_through_a_fixed_clip_is_rejected() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
        let later = project.tracks()[0].clips[1].id;
        let project = project
            .resize_clip(
                track,
                later,
                Tick::quarters(8).unwrap(),
                Tick::quarters(4).unwrap(),
            )
            .unwrap();
        let spanning = NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::quarters(4).unwrap(),
            length: Tick::quarters(5).unwrap(),
        };
        assert_eq!(
            project.insert_note(track, spanning).unwrap_err(),
            ModelError::Overlap
        );
        assert_eq!(project.tracks()[0].clips.len(), 2);
    }

    #[test]
    fn three_four_uses_a_three_quarter_bar() {
        let meter = Meter::try_new(3, 4).unwrap();
        let project = Project::new().with_meter(meter).unwrap();
        let (project, track) = project.add_track("piano");
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        assert_eq!(only_clip(&project).length.get(), TICKS_PER_QUARTER * 3);
        let (project, _) = project.insert_note(track, at(3, 1)).unwrap();
        assert_eq!(only_clip(&project).length.get(), TICKS_PER_QUARTER * 6);
    }

    #[test]
    fn a_note_before_the_origin_occupies_the_previous_bar() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(-1, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.start, Tick::quarters(-4).unwrap());
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(
            clip.absolute_start(&clip.notes[0]).unwrap(),
            Tick::quarters(-1).unwrap()
        );
    }

    #[test]
    fn edits_keep_the_previous_snapshot_and_share_other_tracks() {
        let (project, edited) = project_with_track();
        let (project, other) = project.add_track("drums");
        let shared = project
            .tracks()
            .iter()
            .find(|track| track.id == other)
            .unwrap();
        let (next, _) = project.insert_note(edited, at(0, 1)).unwrap();
        assert!(project.tracks()[0].clips.is_empty());
        assert_eq!(
            next.tracks()
                .iter()
                .find(|track| track.id == edited)
                .unwrap()
                .clips
                .len(),
            1
        );
        let still = next
            .tracks()
            .iter()
            .find(|track| track.id == other)
            .unwrap();
        assert!(Arc::ptr_eq(shared, still));
        assert!(next.delete_note(edited, Id::generate()).is_err());
    }

    #[test]
    fn invalid_notes_and_a_locked_meter_are_rejected() {
        let (project, track) = project_with_track();
        let mut draft = at(0, 1);
        draft.pitch = 128;
        assert_eq!(
            project.insert_note(track, draft).unwrap_err(),
            ModelError::InvalidNote
        );
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        assert_eq!(
            project
                .with_meter(Meter::try_new(3, 4).unwrap())
                .unwrap_err(),
            ModelError::InvalidMeter
        );
    }

    #[test]
    fn assembled_auto_clips_fit_their_notes() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let note = Note {
            id: Id::from_raw(3),
            start: Tick::new(0),
            length: Tick::quarters(1).unwrap(),
            pitch: 60,
            velocity: 100,
        };
        let project = Project::try_assemble(
            Meter::four_four(),
            tempo,
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                clips: vec![
                    Clip {
                        id: Id::from_raw(2),
                        start: Tick::new(0),
                        length: Tick::quarters(8).unwrap(),
                        kind: ClipKind::Auto,
                        notes: vec![note],
                    },
                    Clip {
                        id: Id::from_raw(4),
                        start: Tick::quarters(8).unwrap(),
                        length: Tick::quarters(4).unwrap(),
                        kind: ClipKind::Auto,
                        notes: Vec::new(),
                    },
                ],
            }],
        )
        .unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.id, Id::from_raw(2));
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(clip.absolute_start(&clip.notes[0]).unwrap().get(), 0);
    }

    #[test]
    fn assembled_clips_refuse_overlap_and_duplicate_ids() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let clip = |id, start| Clip {
            id: Id::from_raw(id),
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(4).unwrap(),
            kind: ClipKind::Fixed,
            notes: Vec::new(),
        };
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                clips: vec![clip(2, 0), clip(3, 0)],
            }],
        )
        .unwrap_err();
        assert_eq!(error, ModelError::Overlap);
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo,
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                clips: vec![clip(1, 0)],
            }],
        )
        .unwrap_err();
        assert_eq!(error, ModelError::DuplicateId);
    }

    #[test]
    fn caller_chosen_ids_are_kept_and_reused_ids_are_rejected() {
        let (project, track) = project_with_track();
        let note = Id::from_raw(1);
        let clip = Id::from_raw(2);
        let project = project
            .insert_note_with_id(track, note, clip, at(0, 1))
            .unwrap();
        let stored = only_clip(&project);
        assert_eq!(stored.id, clip);
        assert_eq!(stored.notes[0].id, note);

        let joined = project
            .insert_note_with_id(track, Id::from_raw(4), clip, at(1, 1))
            .unwrap();
        assert_eq!(only_clip(&joined).notes.len(), 2);

        assert_eq!(
            project
                .insert_note_with_id(track, note, Id::from_raw(3), at(1, 1))
                .unwrap_err(),
            ModelError::DuplicateId
        );
        assert_eq!(
            project
                .insert_note_with_id(track, Id::from_raw(5), clip, at(8, 1))
                .unwrap_err(),
            ModelError::DuplicateId
        );
    }

    #[test]
    fn resizing_preserves_timeline_position_when_the_start_moves() {
        let (project, track) = project_with_track();
        let (project, note) = project.insert_note(track, at(1, 1)).unwrap();
        let clip_id = project.tracks()[0].clips[0].id;
        let project = project
            .resize_clip(
                track,
                clip_id,
                Tick::quarters(1).unwrap(),
                Tick::quarters(4).unwrap(),
            )
            .unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.kind, ClipKind::Fixed);
        let kept = clip.notes.iter().find(|item| item.id == note).unwrap();
        assert_eq!(
            clip.absolute_start(kept).unwrap(),
            Tick::quarters(1).unwrap()
        );
    }

    #[test]
    fn a_child_is_placed_after_the_folders_subtree() {
        let (project, folder) = Project::new().add_track("drums");
        let (project, later) = project.add_track("vox");
        let (project, kick) = project.add_child(folder, "kick").unwrap();
        let (project, room) = project.add_child(kick, "room").unwrap();
        let (project, snare) = project.add_child(folder, "snare").unwrap();
        let ids: Vec<_> = project.tracks().iter().map(|track| track.id).collect();
        assert_eq!(ids, vec![folder, kick, room, snare, later]);
        assert_eq!(project.children(folder).unwrap(), vec![kick, snare]);
        assert_eq!(project.parent(room).unwrap(), Some(kick));
        assert_eq!(project.parent(later).unwrap(), None);
    }

    #[test]
    fn a_track_cannot_move_into_its_own_subtree() {
        let (project, folder) = Project::new().add_track("drums");
        let (project, kick) = project.add_child(folder, "kick").unwrap();
        assert_eq!(
            project.set_parent(folder, Some(kick)).unwrap_err(),
            ModelError::Cycle
        );
        assert_eq!(
            project.set_parent(folder, Some(folder)).unwrap_err(),
            ModelError::Cycle
        );
        assert_eq!(
            project.add_child(Id::from_raw(9), "missing").unwrap_err(),
            ModelError::MissingTrack
        );
        let project = project.set_parent(kick, None).unwrap();
        assert_eq!(project.parent(kick).unwrap(), None);
        assert_eq!(project.tracks()[1].id, kick);
    }

    #[test]
    fn saved_folders_reject_a_cycle_and_a_missing_parent() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let bare = |id: u128, parent: Option<u128>| Track {
            id: Id::from_raw(id),
            name: "track".into(),
            parent: parent.map(Id::from_raw),
            clips: Vec::new(),
        };
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            vec![bare(1, Some(2)), bare(2, Some(1))],
        )
        .unwrap_err();
        assert_eq!(error, ModelError::Cycle);
        let error =
            Project::try_assemble(Meter::four_four(), tempo, vec![bare(1, Some(9))]).unwrap_err();
        assert_eq!(error, ModelError::MissingTrack);
    }

    /// 64 is only how far this test walks. The model does not cap folder depth.
    #[test]
    fn a_folder_can_nest_without_a_fixed_depth_cap() {
        let (mut project, mut parent) = Project::new().add_track("root");
        for level in 0..64 {
            let (next, child) = project.add_child(parent, format!("level {level}")).unwrap();
            project = next;
            parent = child;
        }
        assert_eq!(project.tracks().len(), 65);
        let mut cursor = project.parent(parent).unwrap();
        let mut depth = 0;
        while cursor.is_some() {
            depth += 1;
            cursor = project.parent(cursor.unwrap()).unwrap();
        }
        assert_eq!(depth, 64);
    }
}
