//! Typeless tracks and smart clips.
//!
//! A track has no instrument-or-audio kind. An `auto` clip is the smallest run
//! of bars that contains its notes, and it grows into a neighboring bar when a
//! note is placed there. Editing the clip's bounds makes it `fixed`, after
//! which notes outside it start a new `auto` clip.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use fractional_index::FractionalIndex;

use crate::device::{Device, MixerIds, validate_mixer};

use crate::id::Id;
use crate::time::{Meter, Tempo, TempoMap, Tick};

/// Why a model edit was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    InvalidChange,
    InvalidGain,
    InvalidFade,
    Unsorted,
    BadOrder,
    MissingParameter,
    InvalidParameter,
    /// [`Project::check_invariants`] failed. The string is that function's sentence.
    Invariant(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Invariant(message) => return formatter.write_str(message),
            Self::InvalidNote => "note pitch, velocity, or length is not usable",
            Self::InvalidClip => "clip bounds are empty or out of range",
            Self::InvalidMeter => "meter cannot change while a track has clips",
            Self::MissingTrack => "track is not in the project",
            Self::MissingClip => "clip is not on the track",
            Self::MissingNote => "note is not on the track",
            Self::DuplicateId => "id is already used in the project",
            Self::Overlap => "clip would overlap another clip",
            Self::Cycle => "a track cannot contain itself",
            Self::InvalidChange => "changeset does not apply",
            Self::InvalidGain => "gain must be finite and at least zero",
            Self::InvalidFade => "fade is negative or does not fit in a frame count",
            Self::Unsorted => "notes are not in start, pitch, id order",
            Self::BadOrder => "track order key is missing, duplicated, or out of arrange order",
            Self::MissingParameter => "parameter is not in the project",
            Self::InvalidParameter => "parameter value is outside its range",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ModelError {}

/// A note placed on the project timeline, before it belongs to a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoteDraft {
    pub pitch: u8,
    pub velocity: u8,
    pub start: Tick,
    pub length: Tick,
}

/// Geometry of an existing note, in the timeline of the selected placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteGeometry {
    pub pitch: u8,
    pub start: Tick,
    pub length: Tick,
}

/// A note stored relative to its content.
///
/// `velocity` and `release` are 0..=65535. A MIDI 1.0 value `v` in 0..=127 is
/// stored as `(v << 9) | (v << 2) | (v >> 5)`, so 127 becomes 65535.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: Id,
    pub start: Tick,
    pub length: Tick,
    pub pitch: u8,
    pub velocity: u16,
    pub release: u16,
    pub articulation: Option<Id>,
}

/// Stores a 0..=127 velocity in 16 bits. 127 becomes 65535.
pub fn midi7_to_velocity(midi: u8) -> u16 {
    let midi = u16::from(midi);
    (midi << 9) | (midi << 2) | (midi >> 5)
}

/// The top 7 bits of a stored velocity, in 0..=127.
pub fn velocity_to_midi7(velocity: u16) -> u8 {
    (velocity >> 9) as u8
}

/// A reserved per-note curve. Phase 1 only accepts an empty list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionKind {
    Pitch,
    Timbre,
    Pressure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionPoint {
    pub at: Tick,
    pub value: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expression {
    pub note: Id,
    pub kind: ExpressionKind,
    pub points: Vec<ExpressionPoint>,
}

/// Whether a placement still follows its notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Auto,
    Fixed,
}

/// Where a placement sits. Phase 1 has the timeline only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Timeline,
}

/// Five milliseconds, the default edge fade.
pub const DEFAULT_FADE_FLICKS: i64 = 3_528_000;

/// Sample rate used to reject a fade that cannot become a `u32` frame count.
pub const FADE_RATE_LIMIT: u32 = 768_000;

/// Notes shared by one or more placements.
///
/// Content time zero is fixed when the content is created. A note before that
/// origin has a negative `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub id: Id,
    pub audio: Option<Id>,
    pub takes: Vec<Id>,
    pub notes: Vec<Note>,
    /// Per-note expression curves. Empty until those curves can be edited.
    pub expressions: Vec<Expression>,
}

/// A window onto a [`Content`].
///
/// `gain` uses float bits for equality so a project can stay `Eq`.
#[derive(Clone, Debug)]
pub struct Placement {
    pub id: Id,
    pub content: Id,
    pub lane: Lane,
    pub start: Tick,
    pub length: Tick,
    pub offset: Tick,
    pub kind: ClipKind,
    pub gain: f32,
    pub fade_in: i64,
    pub fade_out: i64,
}

impl PartialEq for Placement {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.content == other.content
            && self.lane == other.lane
            && self.start == other.start
            && self.length == other.length
            && self.offset == other.offset
            && self.kind == other.kind
            && self.gain.to_bits() == other.gain.to_bits()
            && self.fade_in == other.fade_in
            && self.fade_out == other.fade_out
    }
}

impl Eq for Placement {}

impl Placement {
    /// Timeline time of a note heard through this window.
    pub fn sounding_start(&self, note: &Note) -> Option<Tick> {
        self.start
            .get()
            .checked_sub(self.offset.get())
            .and_then(|origin| origin.checked_add(note.start.get()))
            .map(Tick::new)
    }

    /// Ticks where this note sounds, clipped to the window. `None` when it misses.
    ///
    /// Both the note and the window are half-open. The second tick is the note-off.
    /// Arithmetic only, so the audio thread can call it. MOD-5.
    pub fn audible(&self, note: &Note) -> Result<Option<(Tick, Tick)>, ModelError> {
        let sound_on = self.sounding_start(note).ok_or(ModelError::InvalidNote)?;
        let sound_off = sound_on
            .get()
            .checked_add(note.length.get())
            .ok_or(ModelError::InvalidNote)?;
        let win_off = self
            .start
            .get()
            .checked_add(self.length.get())
            .ok_or(ModelError::InvalidClip)?;
        let on = sound_on.get().max(self.start.get());
        let off = sound_off.min(win_off);
        Ok((on < off).then_some((Tick::new(on), Tick::new(off))))
    }

    pub(crate) fn new_auto(id: Id, content: Id, start: Tick, length: Tick) -> Self {
        Self {
            id,
            content,
            lane: Lane::Timeline,
            start,
            length,
            offset: Tick::new(0),
            kind: ClipKind::Auto,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        }
    }
}

/// Frames a fade lasts at `sample_rate`, or `None` when it does not fit in `u32`.
///
/// The product is an `i128`. Division is toward zero. Not called from the audio thread.
pub fn fade_frames(flicks: i64, sample_rate: u32) -> Option<u32> {
    if flicks < 0 {
        return None;
    }
    let frames =
        i128::from(flicks) * i128::from(sample_rate) / i128::from(crate::time::FLICKS_PER_SECOND);
    u32::try_from(frames).ok()
}

pub fn fade_allowed(flicks: i64) -> bool {
    flicks >= 0 && fade_frames(flicks, FADE_RATE_LIMIT).is_some()
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
    /// Sibling order under `parent`. Compared as a fractional index string.
    pub order: String,
    /// Placements on this track, in timeline order.
    pub clips: Vec<Placement>,
    /// Built-in devices. A track has one mixer.
    pub devices: Vec<crate::device::Device>,
}

/// Immutable project snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    meter: Meter,
    tempo: TempoMap,
    contents: Vec<Arc<Content>>,
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
            contents: Vec::new(),
            tracks: Vec::new(),
        }
    }

    pub fn contents(&self) -> &[Arc<Content>] {
        &self.contents
    }

    pub fn content(&self, id: Id) -> Option<&Content> {
        self.contents
            .iter()
            .find(|content| content.id == id)
            .map(Arc::as_ref)
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
            contents: self.contents.clone(),
            tracks: self.tracks.clone(),
        })
    }

    #[must_use]
    pub fn with_tempo(&self, at: Tick, tempo: Tempo) -> Self {
        Self {
            meter: self.meter,
            tempo: self.tempo.with_tempo(at, tempo),
            contents: self.contents.clone(),
            tracks: self.tracks.clone(),
        }
    }

    #[must_use]
    pub fn add_track(&self, name: impl Into<String>) -> (Self, Id) {
        let name = name.into();
        let id = Id::generate();
        match self.add_track_with_id(id, name, MixerIds::generate()) {
            Ok(project) => (project, id),
            Err(error) => unreachable!("a generated track id is unique, but {error} was returned"),
        }
    }

    /// Adds a track with an id chosen by the caller, so a journal can replay it.
    pub fn add_track_with_id(
        &self,
        id: Id,
        name: impl Into<String>,
        mixer: MixerIds,
    ) -> Result<Self, ModelError> {
        if self.id_in_use(id)
            || !crate::device::mixer_ids_free(mixer, |candidate| {
                self.id_in_use(candidate) || candidate == id
            })
        {
            return Err(ModelError::DuplicateId);
        }
        let order = next_order(&self.tracks, None)?;
        let mut tracks = self.tracks.clone();
        tracks.push(Arc::new(Track {
            id,
            name: name.into(),
            parent: None,
            order,
            clips: Vec::new(),
            devices: vec![Device::unity(mixer)],
        }));
        Ok(Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents: self.contents.clone(),
            tracks: arrange_tracks(&tracks)?,
        })
    }

    pub fn insert_note(&self, track_id: Id, draft: NoteDraft) -> Result<(Self, Id), ModelError> {
        let note_id = Id::generate();
        let clip_id = Id::generate();
        let content_id = Id::generate();
        self.insert_note_with_id(track_id, note_id, clip_id, content_id, draft)
            .map(|project| (project, note_id))
    }

    /// Adds a track inside `parent`, after that folder's current subtree.
    pub fn add_child(&self, parent: Id, name: impl Into<String>) -> Result<(Self, Id), ModelError> {
        let id = Id::generate();
        self.add_child_with_id(id, parent, name, MixerIds::generate())
            .map(|project| (project, id))
    }

    /// Adds a child with an id chosen by the caller, so a journal can replay it.
    pub fn add_child_with_id(
        &self,
        id: Id,
        parent: Id,
        name: impl Into<String>,
        mixer: MixerIds,
    ) -> Result<Self, ModelError> {
        if self.id_in_use(id)
            || !crate::device::mixer_ids_free(mixer, |candidate| {
                self.id_in_use(candidate) || candidate == id
            })
        {
            return Err(ModelError::DuplicateId);
        }
        self.track_index(parent)?;
        let order = next_order(&self.tracks, Some(parent))?;
        let mut tracks = self.tracks.clone();
        tracks.push(Arc::new(Track {
            id,
            name: name.into(),
            parent: Some(parent),
            order,
            clips: Vec::new(),
            devices: vec![Device::unity(mixer)],
        }));
        Ok(Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents: self.contents.clone(),
            tracks: arrange_tracks(&tracks)?,
        })
    }

    /// Moves a track under another folder, or to the top level when `parent` is `None`.
    ///
    /// The track becomes the last sibling there. Its descendants keep their
    /// parents and order keys, and the subtree moves together. A track cannot
    /// move into its own subtree.
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
        let order = next_order(&self.tracks, parent)?;
        let mut tracks = self.tracks.clone();
        let mut track = (*tracks[index]).clone();
        track.parent = parent;
        track.order = order;
        tracks[index] = Arc::new(track);
        Ok(Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents: self.contents.clone(),
            tracks: arrange_tracks(&tracks)?,
        })
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
    /// `clip_id` and `content_id` are used only when this note creates a placement.
    pub fn insert_note_with_id(
        &self,
        track_id: Id,
        note_id: Id,
        clip_id: Id,
        content_id: Id,
        draft: NoteDraft,
    ) -> Result<Self, ModelError> {
        validate_note(&draft)?;
        if self.id_in_use(note_id) {
            return Err(ModelError::DuplicateId);
        }
        let index = self.track_index(track_id)?;
        let bar = self
            .meter
            .bar_ticks()
            .ok_or(ModelError::InvalidMeter)?
            .get();
        let clip_free = clip_id != note_id
            && clip_id != content_id
            && note_id != content_id
            && !self.id_in_use(clip_id);
        let content_free = !self.id_in_use(content_id);
        let mut contents = self.contents.clone();
        let mut tracks = self.tracks.clone();
        let mut track = (*tracks[index]).clone();
        crate::placement::insert(
            &mut contents,
            &mut tracks,
            &mut track,
            crate::placement::FreshIds {
                note: note_id,
                clip: clip_id,
                content: content_id,
                clip_free,
                content_free,
            },
            &draft,
            bar,
        )?;
        sort_clips(&mut track.clips);
        tracks[index] = Arc::new(track);
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    pub fn delete_note(&self, track_id: Id, note_id: Id) -> Result<Self, ModelError> {
        let index = self.track_index(track_id)?;
        let bar = self
            .meter
            .bar_ticks()
            .ok_or(ModelError::InvalidMeter)?
            .get();
        let mut contents = self.contents.clone();
        let mut tracks = self.tracks.clone();
        let mut track = (*tracks[index]).clone();
        crate::placement::delete(&mut contents, &mut tracks, &mut track, note_id, bar)?;
        tracks[index] = Arc::new(track);
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    /// Edits shared content without replacing the note id or its expression data.
    /// Auto placements follow the result; fixed windows keep their boundaries.
    pub fn edit_note(
        &self,
        track_id: Id,
        clip_id: Id,
        note_id: Id,
        geometry: NoteGeometry,
    ) -> Result<Self, ModelError> {
        validate_note(&NoteDraft {
            pitch: geometry.pitch,
            velocity: 0,
            start: geometry.start,
            length: geometry.length,
        })?;
        let index = self.track_index(track_id)?;
        let bar = self
            .meter
            .bar_ticks()
            .ok_or(ModelError::InvalidMeter)?
            .get();
        let mut contents = self.contents.clone();
        let mut tracks = self.tracks.clone();
        let mut track = (*tracks[index]).clone();
        crate::placement::edit(
            &mut contents,
            &mut tracks,
            &mut track,
            clip_id,
            note_id,
            geometry,
            bar,
        )?;
        sort_clips(&mut track.clips);
        tracks[index] = Arc::new(track);
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    /// Moves a placement's window and marks it fixed. Sounding times stay put.
    pub fn resize_clip(
        &self,
        track_id: Id,
        clip_id: Id,
        start: Tick,
        length: Tick,
    ) -> Result<Self, ModelError> {
        if length.get() <= 0 || start.get().checked_add(length.get()).is_none() {
            return Err(ModelError::InvalidClip);
        }
        let index = self.track_index(track_id)?;
        let mut track = (*self.tracks[index]).clone();
        let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) else {
            return Err(ModelError::MissingClip);
        };
        let delta = start
            .get()
            .checked_sub(clip.start.get())
            .ok_or(ModelError::InvalidClip)?;
        clip.offset = Tick::new(
            clip.offset
                .get()
                .checked_add(delta)
                .ok_or(ModelError::InvalidClip)?,
        );
        clip.start = start;
        clip.length = length;
        clip.kind = ClipKind::Fixed;
        sort_clips(&mut track.clips);
        let position = track
            .clips
            .iter()
            .position(|clip| clip.id == clip_id)
            .ok_or(ModelError::MissingClip)?;
        if overlaps_others(&track.clips, position) {
            return Err(ModelError::Overlap);
        }
        Ok(self.write_track(index, track))
    }

    /// A fixed window onto an existing content.
    pub fn link_clip(
        &self,
        source: Id,
        track_id: Id,
        start: Tick,
        clip_id: Id,
    ) -> Result<Self, ModelError> {
        if self.id_in_use(clip_id) {
            return Err(ModelError::DuplicateId);
        }
        let source = self
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.id == source)
            .cloned()
            .ok_or(ModelError::MissingClip)?;
        if start.get().checked_add(source.length.get()).is_none() || source.length.get() <= 0 {
            return Err(ModelError::InvalidClip);
        }
        let index = self.track_index(track_id)?;
        let mut track = (*self.tracks[index]).clone();
        track.clips.push(Placement {
            id: clip_id,
            content: source.content,
            lane: Lane::Timeline,
            start,
            length: source.length,
            offset: source.offset,
            kind: ClipKind::Fixed,
            gain: source.gain,
            fade_in: source.fade_in,
            fade_out: source.fade_out,
        });
        sort_clips(&mut track.clips);
        let position = track
            .clips
            .iter()
            .position(|clip| clip.id == clip_id)
            .ok_or(ModelError::MissingClip)?;
        if overlaps_others(&track.clips, position) {
            return Err(ModelError::Overlap);
        }
        Ok(self.write_track(index, track))
    }

    pub fn set_clip_gain(&self, clip_id: Id, gain: f32) -> Result<Self, ModelError> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(ModelError::InvalidGain);
        }
        self.edit_clip(clip_id, |clip| clip.gain = gain)
    }

    pub fn set_clip_fades(
        &self,
        clip_id: Id,
        fade_in: i64,
        fade_out: i64,
    ) -> Result<Self, ModelError> {
        if !fade_allowed(fade_in) || !fade_allowed(fade_out) {
            return Err(ModelError::InvalidFade);
        }
        self.edit_clip(clip_id, |clip| {
            clip.fade_in = fade_in;
            clip.fade_out = fade_out;
        })
    }

    fn edit_clip(
        &self,
        clip_id: Id,
        edit: impl FnOnce(&mut Placement),
    ) -> Result<Self, ModelError> {
        let Some(index) = self
            .tracks
            .iter()
            .position(|track| track.clips.iter().any(|clip| clip.id == clip_id))
        else {
            return Err(ModelError::MissingClip);
        };
        let mut track = (*self.tracks[index]).clone();
        let clip = track
            .clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or(ModelError::MissingClip)?;
        edit(clip);
        Ok(self.write_track(index, track))
    }

    fn with_parts(&self, contents: Vec<Arc<Content>>, tracks: Vec<Arc<Track>>) -> Self {
        Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents,
            tracks,
        }
    }

    fn track_index(&self, id: Id) -> Result<usize, ModelError> {
        self.tracks
            .iter()
            .position(|track| track.id == id)
            .ok_or(ModelError::MissingTrack)
    }

    fn id_in_use(&self, id: Id) -> bool {
        self.contents.iter().any(|content| {
            content.id == id
                || content.audio == Some(id)
                || content.takes.contains(&id)
                || content.notes.iter().any(|note| note.id == id)
        }) || self.tracks.iter().any(|track| {
            track.id == id
                || track.clips.iter().any(|clip| clip.id == id)
                || track.devices.iter().any(|device| {
                    device.id == id || device.parameters.iter().any(|parameter| parameter.id == id)
                })
        })
    }

    pub fn set_parameter(&self, id: Id, value: f32) -> Result<Self, ModelError> {
        let Some((track_index, device_index, parameter_index)) = self.locate_parameter(id) else {
            return Err(ModelError::MissingParameter);
        };
        let parameter = &self.tracks[track_index].devices[device_index].parameters[parameter_index];
        if !parameter.key.allows(value) {
            return Err(ModelError::InvalidParameter);
        }
        if parameter.value.to_bits() == value.to_bits() {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[track_index]).clone();
        track.devices[device_index].parameters[parameter_index].value = value;
        Ok(self.write_track(track_index, track))
    }

    pub fn parameter(&self, id: Id) -> Option<&crate::device::Parameter> {
        let (track_index, device_index, parameter_index) = self.locate_parameter(id)?;
        Some(&self.tracks[track_index].devices[device_index].parameters[parameter_index])
    }

    fn locate_parameter(&self, id: Id) -> Option<(usize, usize, usize)> {
        for (track_index, track) in self.tracks.iter().enumerate() {
            for (device_index, device) in track.devices.iter().enumerate() {
                if let Some(parameter_index) = device
                    .parameters
                    .iter()
                    .position(|parameter| parameter.id == id)
                {
                    return Some((track_index, device_index, parameter_index));
                }
            }
        }
        None
    }

    /// Builds a project from saved parts.
    ///
    /// Ids must be unique. Parts are stored as given; note starts are not rewritten.
    pub fn try_assemble(
        meter: Meter,
        tempo: TempoMap,
        mut contents: Vec<Content>,
        tracks: Vec<Track>,
    ) -> Result<Self, ModelError> {
        let _ = meter.bar_ticks().ok_or(ModelError::InvalidMeter)?;
        contents.sort_by_key(|content| content.id);
        let mut seen = HashSet::new();
        let mut stored = Vec::with_capacity(contents.len());
        for content in contents {
            if !seen.insert(content.id) || content.takes.contains(&content.id) {
                return Err(ModelError::DuplicateId);
            }
            if !content.expressions.is_empty() {
                return Err(ModelError::InvalidNote);
            }
            for note in &content.notes {
                if !seen.insert(note.id) {
                    return Err(ModelError::DuplicateId);
                }
                if note.pitch > 127
                    || note.length.get() <= 0
                    || note.articulation.is_some()
                    || note.start.get().checked_add(note.length.get()).is_none()
                {
                    return Err(ModelError::InvalidNote);
                }
            }
            if !notes_in_order(&content.notes) {
                return Err(ModelError::Unsorted);
            }
            stored.push(Arc::new(content));
        }
        let mut assembled = Vec::with_capacity(tracks.len());
        for track in tracks {
            if !seen.insert(track.id) {
                return Err(ModelError::DuplicateId);
            }
            for clip in &track.clips {
                if !seen.insert(clip.id) {
                    return Err(ModelError::DuplicateId);
                }
                if clip.length.get() <= 0
                    || clip.start.get().checked_add(clip.length.get()).is_none()
                {
                    return Err(ModelError::InvalidClip);
                }
                if !fade_allowed(clip.fade_in) || !fade_allowed(clip.fade_out) {
                    return Err(ModelError::InvalidFade);
                }
                if !clip.gain.is_finite() || clip.gain < 0.0 {
                    return Err(ModelError::InvalidGain);
                }
                if !stored.iter().any(|content| content.id == clip.content) {
                    return Err(ModelError::MissingClip);
                }
            }
            if track
                .clips
                .iter()
                .enumerate()
                .any(|(index, _)| overlaps_others(&track.clips, index))
            {
                return Err(ModelError::Overlap);
            }
            if track.devices.len() != 1 {
                return Err(ModelError::InvalidParameter);
            }
            for device in &track.devices {
                validate_mixer(device, &mut seen)?;
            }
            assembled.push(Arc::new(track));
        }
        validate_forest(&assembled)?;
        if arrange_fault(&assembled).is_some() {
            return Err(ModelError::BadOrder);
        }
        if stored.iter().any(|content| {
            !assembled
                .iter()
                .any(|track| track.clips.iter().any(|clip| clip.content == content.id))
        }) {
            return Err(ModelError::InvalidClip);
        }
        let project = Self {
            meter,
            tempo,
            contents: stored,
            tracks: assembled,
        };
        project
            .check_invariants()
            .map_err(|error| ModelError::Invariant(error.to_string()))?;
        Ok(project)
    }

    fn write_track(&self, index: usize, track: Track) -> Self {
        let mut tracks = self.tracks.clone();
        tracks[index] = Arc::new(track);
        Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents: self.contents.clone(),
            tracks,
        }
    }

    pub(crate) fn from_arcs(
        meter: Meter,
        tempo: TempoMap,
        contents: Vec<Arc<Content>>,
        tracks: Vec<Arc<Track>>,
    ) -> Self {
        Self {
            meter,
            tempo,
            contents,
            tracks,
        }
    }

    /// Builds a snapshot without any checks, so a test can break a rule on purpose.
    #[cfg(test)]
    pub(crate) fn unchecked(
        meter: Meter,
        tempo: TempoMap,
        contents: Vec<Content>,
        tracks: Vec<Track>,
    ) -> Self {
        Self {
            meter,
            tempo,
            contents: contents.into_iter().map(Arc::new).collect(),
            tracks: tracks.into_iter().map(Arc::new).collect(),
        }
    }
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

/// Fractional index of the first sibling in a group.
pub fn initial_order() -> String {
    sibling_order(0)
}

/// `index` steps after [`initial_order`]. Zero is the first sibling.
pub(crate) fn sibling_order(index: u32) -> String {
    let mut key = FractionalIndex::default();
    for _ in 0..index {
        key = FractionalIndex::new_after(&key);
    }
    key.to_string()
}

fn next_order(tracks: &[Arc<Track>], parent: Option<Id>) -> Result<String, ModelError> {
    let mut last: Option<&str> = None;
    for track in tracks {
        if track.parent != parent {
            continue;
        }
        if last.is_none_or(|current| track.order.as_str() > current) {
            last = Some(track.order.as_str());
        }
    }
    let next = match last {
        Some(text) => {
            let key = FractionalIndex::from_string(text).map_err(|_| ModelError::BadOrder)?;
            FractionalIndex::new_after(&key)
        }
        None => FractionalIndex::default(),
    };
    Ok(next.to_string())
}

/// Arrange order from sibling keys. The walk is iterative, so depth is not capped.
pub(crate) fn arrange_tracks(tracks: &[Arc<Track>]) -> Result<Vec<Arc<Track>>, ModelError> {
    derived_order(tracks).map_err(|_| ModelError::BadOrder)
}

/// `None` when `tracks` is already the arrange order. Otherwise words for the first fault.
pub(crate) fn arrange_fault(tracks: &[Arc<Track>]) -> Option<String> {
    match derived_order(tracks) {
        Err(message) => Some(message),
        Ok(ordered) => tracks
            .iter()
            .zip(ordered)
            .find(|(given, want)| given.id != want.id)
            .map(|(given, _)| format!("track {} is out of arrange order", given.name)),
    }
}

fn derived_order(tracks: &[Arc<Track>]) -> Result<Vec<Arc<Track>>, String> {
    let mut groups: HashMap<Option<Id>, Vec<usize>> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        if FractionalIndex::from_string(&track.order).is_err() {
            return Err(format!(
                "track {} has an order key that is not a fractional index",
                track.name
            ));
        }
        groups.entry(track.parent).or_default().push(index);
    }
    for group in groups.values_mut() {
        group.sort_by(|&left, &right| tracks[left].order.cmp(&tracks[right].order));
        for pair in group.windows(2) {
            if tracks[pair[0]].order == tracks[pair[1]].order {
                return Err(format!(
                    "tracks {} and {} share an order key",
                    tracks[pair[0]].name, tracks[pair[1]].name
                ));
            }
        }
    }
    let mut stack = groups.get(&None).cloned().unwrap_or_default();
    stack.reverse();
    let mut ordered = Vec::with_capacity(tracks.len());
    let mut seen = HashSet::new();
    while let Some(index) = stack.pop() {
        let id = tracks[index].id;
        if !seen.insert(id) {
            return Err(format!("track {} is its own ancestor", tracks[index].name));
        }
        ordered.push(Arc::clone(&tracks[index]));
        if let Some(children) = groups.get(&Some(id)) {
            for &child in children.iter().rev() {
                stack.push(child);
            }
        }
    }
    if ordered.len() != tracks.len() {
        let missing = tracks
            .iter()
            .find(|track| !seen.contains(&track.id))
            .unwrap();
        return Err(format!(
            "track {} is not in the arrange order",
            missing.name
        ));
    }
    Ok(ordered)
}

/// MOD-4: content-local start, then pitch, then the numeric id.
pub(crate) fn note_ord(left: &Note, right: &Note) -> std::cmp::Ordering {
    left.start
        .cmp(&right.start)
        .then(left.pitch.cmp(&right.pitch))
        .then(left.id.cmp(&right.id))
}

pub(crate) fn notes_in_order(notes: &[Note]) -> bool {
    notes
        .windows(2)
        .all(|pair| note_ord(&pair[0], &pair[1]).is_le())
}

pub(crate) fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(note_ord);
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

fn span(clip: &Placement) -> Option<(i64, i64)> {
    clip.start
        .get()
        .checked_add(clip.length.get())
        .map(|end| (clip.start.get(), end))
}

pub(crate) fn overlaps_others(clips: &[Placement], index: usize) -> bool {
    let Some(range) = span(&clips[index]) else {
        return true;
    };
    clips.iter().enumerate().any(|(other_index, other)| {
        other_index != index
            && span(other).is_some_and(|other_range| spans_overlap(range, other_range))
    })
}

pub(crate) fn overlaps_span(clips: &[Placement], start: i64, end: i64) -> bool {
    clips
        .iter()
        .any(|clip| span(clip).is_some_and(|range| spans_overlap(range, (start, end))))
}

fn spans_overlap(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

fn clip_bars(clip: &Placement, bar: i64) -> Option<(i64, i64)> {
    occupied_bars(clip.start.get(), clip.length.get(), bar)
}

pub(crate) fn occupied_bars(start: i64, length: i64, bar: i64) -> Option<(i64, i64)> {
    if length <= 0 || bar <= 0 {
        return None;
    }
    let last = start.checked_add(length - 1)?;
    Some((start.div_euclid(bar), last.div_euclid(bar)))
}

pub(crate) fn bars_touch(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 <= right.1.saturating_add(1) && right.0 <= left.1.saturating_add(1)
}

pub(crate) fn fixed_blocks(
    clips: &[Placement],
    left: (i64, i64),
    right: (i64, i64),
    bar: i64,
) -> bool {
    let low = left.0.min(right.0);
    let high = left.1.max(right.1);
    clips.iter().any(|clip| {
        clip.kind == ClipKind::Fixed
            && clip_bars(clip, bar).is_some_and(|bars| bars.0 <= high && low <= bars.1)
    })
}

pub(crate) fn sort_clips(clips: &mut [Placement]) {
    clips.sort_by(|left, right| left.start.cmp(&right.start).then(left.id.cmp(&right.id)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::time::{Meter, TICKS_PER_QUARTER, Tempo, TempoMap, Tick};
    use std::sync::Arc;

    #[test]
    fn midi_velocities_round_trip_through_sixteen_bits() {
        assert_eq!(midi7_to_velocity(127), 65535);
        assert_eq!(midi7_to_velocity(0), 0);
        for midi in 0..=127 {
            assert_eq!(velocity_to_midi7(midi7_to_velocity(midi)), midi);
        }
    }

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

    fn only_clip(project: &Project) -> &Placement {
        assert_eq!(project.tracks()[0].clips.len(), 1);
        &project.tracks()[0].clips[0]
    }

    fn first_note<'a>(project: &'a Project, clip: &Placement) -> &'a Note {
        &project.content(clip.content).expect("content").notes[0]
    }

    fn sounding(project: &Project, clip: &Placement) -> Tick {
        clip.sounding_start(first_note(project, clip)).unwrap()
    }

    #[test]
    fn placing_a_note_creates_one_bar_auto_clip() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(1, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.kind, ClipKind::Auto);
        assert_eq!(clip.start.get(), 0);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(sounding(&project, clip), Tick::quarters(1).unwrap());
    }

    #[test]
    fn the_next_bar_extends_an_auto_clip_without_moving_notes() {
        let (project, track) = project_with_track();
        let (project, first) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 8);
        let kept = project
            .content(clip.content)
            .unwrap()
            .notes
            .iter()
            .find(|note| note.id == first)
            .unwrap();
        assert_eq!(clip.sounding_start(kept).unwrap().get(), 0);
    }

    #[test]
    fn a_gap_starts_a_second_clip_until_the_gap_is_filled() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
        assert_eq!(project.tracks()[0].clips.len(), 2);
        let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
        let clip = only_clip(&project);
        assert_eq!(project.content(clip.content).unwrap().notes.len(), 3);
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
        assert_eq!(sounding(&project, clip).get(), 0);
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
        assert_eq!(sounding(&project, fixed).get(), 0);
        assert_eq!(first_note(&project, fixed).id, note);
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
        assert_eq!(project.content(clip.content).unwrap().notes.len(), 2);
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
        assert_eq!(sounding(&project, clip), Tick::quarters(-1).unwrap());
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
        let content = Content {
            id: Id::from_raw(5),
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(3),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            }],
            expressions: Vec::new(),
        };
        let project = Project::try_assemble(
            Meter::four_four(),
            tempo,
            vec![content],
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                order: sibling_order(0),
                devices: vec![crate::device::test_mixer(1)],
                clips: vec![Placement::new_auto(
                    Id::from_raw(2),
                    Id::from_raw(5),
                    Tick::new(0),
                    Tick::quarters(4).unwrap(),
                )],
            }],
        )
        .unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.id, Id::from_raw(2));
        assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
        assert_eq!(sounding(&project, clip).get(), 0);
    }

    #[test]
    fn assembled_clips_refuse_overlap_and_duplicate_ids() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let clip = |id, content, start| Placement {
            id: Id::from_raw(id),
            content: Id::from_raw(content),
            lane: Lane::Timeline,
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(4).unwrap(),
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        };
        let shell = |id| Content {
            id: Id::from_raw(id),
            audio: None,
            takes: Vec::new(),
            notes: Vec::new(),
            expressions: Vec::new(),
        };
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            vec![shell(20), shell(30)],
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                order: sibling_order(0),
                devices: vec![crate::device::test_mixer(1)],
                clips: vec![clip(2, 20, 0), clip(3, 30, 0)],
            }],
        )
        .unwrap_err();
        assert_eq!(error, ModelError::Overlap);
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo,
            vec![shell(20)],
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                order: sibling_order(0),
                devices: vec![crate::device::test_mixer(1)],
                clips: vec![clip(1, 20, 0)],
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
            .insert_note_with_id(track, note, clip, Id::from_raw(8), at(0, 1))
            .unwrap();
        let stored = only_clip(&project);
        assert_eq!(stored.id, clip);
        assert_eq!(first_note(&project, stored).id, note);

        let joined = project
            .insert_note_with_id(track, Id::from_raw(4), clip, Id::from_raw(9), at(1, 1))
            .unwrap();
        assert_eq!(
            joined
                .content(only_clip(&joined).content)
                .unwrap()
                .notes
                .len(),
            2
        );

        assert_eq!(
            project
                .insert_note_with_id(track, note, Id::from_raw(3), Id::from_raw(10), at(1, 1))
                .unwrap_err(),
            ModelError::DuplicateId
        );
        assert_eq!(
            project
                .insert_note_with_id(track, Id::from_raw(5), clip, Id::from_raw(11), at(8, 1))
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
        let kept = project
            .content(clip.content)
            .unwrap()
            .notes
            .iter()
            .find(|item| item.id == note)
            .unwrap();
        assert_eq!(
            clip.sounding_start(kept).unwrap(),
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
            order: sibling_order(id as u32),
            devices: vec![crate::device::test_mixer(id)],
            clips: Vec::new(),
        };
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            Vec::new(),
            vec![bare(1, Some(2)), bare(2, Some(1))],
        )
        .unwrap_err();
        assert_eq!(error, ModelError::Cycle);
        let error = Project::try_assemble(
            Meter::four_four(),
            tempo,
            Vec::new(),
            vec![bare(1, Some(9))],
        )
        .unwrap_err();
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

    #[test]
    fn a_note_added_through_one_placement_is_visible_from_the_other() {
        let (project, piano) = project_with_track();
        let (project, drums) = project.add_track("drums");
        let (project, first) = project.insert_note(piano, at(0, 1)).unwrap();
        let source = only_clip(&project).id;
        let project = project
            .resize_clip(piano, source, Tick::new(0), Tick::quarters(4).unwrap())
            .unwrap();
        let project = project
            .link_clip(source, drums, Tick::quarters(16).unwrap(), Id::from_raw(9))
            .unwrap();
        let (project, second) = project.insert_note(piano, at(1, 1)).unwrap();
        let piano_clip = project
            .tracks()
            .iter()
            .find(|track| track.id == piano)
            .unwrap()
            .clips
            .first()
            .unwrap();
        let drum_clip = project
            .tracks()
            .iter()
            .find(|track| track.id == drums)
            .unwrap()
            .clips
            .first()
            .unwrap();
        assert_eq!(piano_clip.kind, ClipKind::Fixed);
        assert_eq!(drum_clip.kind, ClipKind::Fixed);
        assert_eq!(piano_clip.content, drum_clip.content);
        let notes = &project.content(drum_clip.content).unwrap().notes;
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].id, first);
        assert_eq!(notes[0].start.get(), 0);
        assert_eq!(notes[1].id, second);
        assert_eq!(
            piano_clip.sounding_start(&notes[1]).unwrap(),
            Tick::quarters(1).unwrap()
        );
        assert_eq!(
            drum_clip.sounding_start(&notes[1]).unwrap(),
            Tick::quarters(17).unwrap()
        );
        project.check_invariants().unwrap();
    }

    fn window(
        id: u128,
        content: u128,
        start: i64,
        length: i64,
        offset: i64,
        kind: ClipKind,
    ) -> Placement {
        Placement {
            id: Id::from_raw(id),
            content: Id::from_raw(content),
            lane: Lane::Timeline,
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(length).unwrap(),
            offset: Tick::quarters(offset).unwrap(),
            kind,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        }
    }

    fn content_with(id: u128, note: u128, start: i64) -> Content {
        Content {
            id: Id::from_raw(id),
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(note),
                start: Tick::quarters(start).unwrap(),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            }],
            expressions: Vec::new(),
        }
    }

    fn shared_across_tracks(blocker: bool) -> (Project, Id, Id) {
        let mut drum_clips = Vec::new();
        let mut contents = vec![content_with(5, 3, 0)];
        if blocker {
            contents.push(content_with(8, 11, 0));
            drum_clips.push(window(7, 8, -4, 4, 0, ClipKind::Fixed));
        }
        drum_clips.push(window(6, 5, 0, 4, 0, ClipKind::Auto));
        let project = Project::try_assemble(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            contents,
            vec![
                Track {
                    id: Id::from_raw(1),
                    name: "piano".into(),
                    parent: None,
                    order: sibling_order(0),
                    devices: vec![crate::device::test_mixer(1)],
                    clips: vec![window(2, 5, -4, 8, -4, ClipKind::Fixed)],
                },
                Track {
                    id: Id::from_raw(4),
                    name: "drums".into(),
                    parent: None,
                    order: sibling_order(1),
                    devices: vec![crate::device::test_mixer(4)],
                    clips: drum_clips,
                },
            ],
        )
        .unwrap();
        project.check_invariants().unwrap();
        (project, Id::from_raw(1), Id::from_raw(3))
    }

    #[test]
    fn an_earlier_note_grows_only_the_auto_on_the_other_track() {
        let (project, piano, kept) = shared_across_tracks(false);
        let (project, _) = project.insert_note(piano, at(-1, 1)).unwrap();
        let content = project.content(Id::from_raw(5)).unwrap();
        let kept_note = content.notes.iter().find(|note| note.id == kept).unwrap();
        assert_eq!(kept_note.start.get(), 0);
        let added = content.notes.iter().find(|note| note.id != kept).unwrap();
        assert_eq!(added.start, Tick::quarters(-1).unwrap());
        let piano_clip = &project.tracks()[0].clips[0];
        assert_eq!(piano_clip.kind, ClipKind::Fixed);
        assert_eq!(piano_clip.start, Tick::quarters(-4).unwrap());
        assert_eq!(piano_clip.length, Tick::quarters(8).unwrap());
        assert_eq!(piano_clip.offset, Tick::quarters(-4).unwrap());
        let auto = &project.tracks()[1].clips[0];
        assert_eq!(auto.kind, ClipKind::Auto);
        assert_eq!(auto.start, Tick::quarters(-4).unwrap());
        assert_eq!(auto.length, Tick::quarters(8).unwrap());
        assert_eq!(auto.offset, Tick::quarters(-4).unwrap());
        assert_eq!(auto.sounding_start(kept_note).unwrap(), Tick::new(0));
        project.check_invariants().unwrap();
    }

    #[test]
    fn growing_the_auto_on_the_other_track_rejects_an_overlap() {
        let (project, piano, _) = shared_across_tracks(true);
        assert_eq!(
            project.insert_note(piano, at(-1, 1)).unwrap_err(),
            ModelError::Overlap
        );
        assert_eq!(project.tracks()[1].clips[1].start.get(), 0);
        assert_eq!(
            project.content(Id::from_raw(5)).unwrap().notes[0]
                .start
                .get(),
            0
        );
    }

    fn fade_past_u32() -> i64 {
        let rate = i128::from(FADE_RATE_LIMIT);
        let frames = i128::from(u32::MAX) + 1;
        let numer = frames * i128::from(crate::time::FLICKS_PER_SECOND);
        i64::try_from((numer + rate - 1) / rate).unwrap()
    }

    #[test]
    fn a_fade_that_does_not_fit_in_u32_frames_is_rejected() {
        assert_eq!(fade_frames(0, 44_100), Some(0));
        assert_eq!(fade_frames(1, 44_100), Some(0));
        assert_eq!(fade_frames(DEFAULT_FADE_FLICKS, 44_100), Some(220));
        let too_wide = fade_past_u32();
        assert!(fade_frames(too_wide, FADE_RATE_LIMIT).is_none());
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let clip = only_clip(&project).id;
        assert_eq!(
            project.set_clip_fades(clip, too_wide, 0).unwrap_err(),
            ModelError::InvalidFade
        );
        let mut assembled = (*project.tracks()[0]).clone();
        assembled.clips[0].fade_in = too_wide;
        assert_eq!(
            Project::try_assemble(
                project.meter(),
                project.tempo().clone(),
                project
                    .contents()
                    .iter()
                    .map(|content| (**content).clone())
                    .collect(),
                vec![assembled],
            )
            .unwrap_err(),
            ModelError::InvalidFade
        );
    }

    #[test]
    fn fades_longer_than_the_placement_are_accepted() {
        let (project, track) = project_with_track();
        let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
        let clip = only_clip(&project).id;
        // Four quarters at 120 bpm last two seconds. Each fade is sixty seconds.
        let fade = crate::time::FLICKS_PER_SECOND * 60;
        let project = project.set_clip_fades(clip, fade, fade).unwrap();
        let clip = only_clip(&project);
        assert_eq!(clip.length, Tick::quarters(4).unwrap());
        assert_eq!(clip.fade_in, fade);
        assert_eq!(clip.fade_out, fade);
        project.check_invariants().unwrap();
    }

    #[test]
    fn an_earlier_note_is_stored_before_the_one_already_there() {
        let (project, track) = project_with_track();
        let (project, later) = project.insert_note(track, at(4, 1)).unwrap();
        let kept = first_note(&project, only_clip(&project)).start;
        let (project, earlier) = project.insert_note(track, at(0, 1)).unwrap();
        let notes = &project.content(only_clip(&project).content).unwrap().notes;
        assert_eq!(notes[0].id, earlier);
        assert_eq!(notes[1].id, later);
        assert_eq!(notes[1].start, kept);
        project.check_invariants().unwrap();
    }

    #[test]
    fn the_lower_pitch_comes_first_when_notes_start_together() {
        let (project, track) = project_with_track();
        let high = NoteDraft {
            pitch: 67,
            ..at(0, 1)
        };
        let low = NoteDraft {
            pitch: 60,
            ..at(0, 1)
        };
        let (project, high_id) = project.insert_note(track, high).unwrap();
        let (project, low_id) = project.insert_note(track, low).unwrap();
        let notes = &project.content(only_clip(&project).content).unwrap().notes;
        assert_eq!(notes[0].id, low_id);
        assert_eq!(notes[1].id, high_id);
        assert_eq!(notes[0].start, notes[1].start);
        assert_eq!(notes[1].pitch, 67);
        project.check_invariants().unwrap();
    }

    #[test]
    fn the_smaller_id_comes_first_when_start_and_pitch_match() {
        let (project, track) = project_with_track();
        let project = project
            .insert_note_with_id(
                track,
                Id::from_raw(20),
                Id::from_raw(2),
                Id::from_raw(9),
                at(0, 1),
            )
            .unwrap();
        let project = project
            .insert_note_with_id(
                track,
                Id::from_raw(10),
                Id::from_raw(3),
                Id::from_raw(11),
                at(0, 1),
            )
            .unwrap();
        let notes = &project.content(only_clip(&project).content).unwrap().notes;
        assert_eq!(notes[0].id, Id::from_raw(10));
        assert_eq!(notes[1].id, Id::from_raw(20));
        assert_eq!(notes[0].start, notes[1].start);
        assert_eq!(notes[0].pitch, notes[1].pitch);
        project.check_invariants().unwrap();
    }

    #[test]
    fn unsorted_notes_are_rejected() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let content = Content {
            id: Id::from_raw(5),
            audio: None,
            takes: Vec::new(),
            notes: vec![
                Note {
                    id: Id::from_raw(4),
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                    pitch: 67,
                    velocity: 100,
                    release: 0,
                    articulation: None,
                },
                Note {
                    id: Id::from_raw(3),
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                    pitch: 60,
                    velocity: 100,
                    release: 0,
                    articulation: None,
                },
            ],
            expressions: Vec::new(),
        };
        let track = Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: sibling_order(0),
            devices: vec![crate::device::test_mixer(1)],
            clips: vec![Placement::new_auto(
                Id::from_raw(2),
                Id::from_raw(5),
                Tick::new(0),
                Tick::quarters(4).unwrap(),
            )],
        };
        assert_eq!(
            Project::try_assemble(
                Meter::four_four(),
                tempo.clone(),
                vec![content.clone()],
                vec![track.clone()]
            )
            .unwrap_err(),
            ModelError::Unsorted
        );
        let stored = Project::unchecked(Meter::four_four(), tempo, vec![content], vec![track]);
        assert!(
            stored
                .check_invariants()
                .unwrap_err()
                .to_string()
                .contains("out of start, pitch, id order")
        );
    }

    #[test]
    fn moving_a_track_under_another_keeps_every_other_order_key() {
        use crate::changeset::{Change, ObjectKey, Property};
        let (project, a) = Project::new().add_track("A");
        let (project, b) = project.add_track("B");
        let (project, _) = project.add_child(a, "a1").unwrap();
        let before = project.clone();
        let orders: Vec<(Id, String)> = project
            .tracks()
            .iter()
            .map(|track| (track.id, track.order.clone()))
            .collect();
        let project = project.set_parent(a, Some(b)).unwrap();
        let names: Vec<_> = project
            .tracks()
            .iter()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(names, ["B", "A", "a1"]);
        for track in project.tracks() {
            if track.id != a {
                let (_, previous) = orders.iter().find(|(id, _)| *id == track.id).unwrap();
                assert_eq!(&track.order, previous);
            }
        }
        let changes = before.diff(&project);
        assert!(changes.changes().iter().all(|change| {
            !matches!(
                change,
                Change::Update {
                    property: Property::TrackOrder,
                    ..
                }
            )
        }));
        let order_ids: Vec<Id> = changes
            .changes()
            .iter()
            .filter_map(|change| match change {
                Change::Update {
                    id: ObjectKey::Id(id),
                    property: Property::Order,
                    ..
                } => Some(*id),
                _ => None,
            })
            .collect();
        assert!(order_ids.iter().all(|id| *id == a));
        project.check_invariants().unwrap();
    }

    #[test]
    fn a_bad_track_order_is_rejected() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let track = |id: u128, order: String| Track {
            id: Id::from_raw(id),
            name: format!("t{id}"),
            parent: None,
            order,
            devices: vec![crate::device::test_mixer(id)],
            clips: Vec::new(),
        };
        let same = sibling_order(0);
        assert_eq!(
            Project::try_assemble(
                Meter::four_four(),
                tempo.clone(),
                Vec::new(),
                vec![track(1, same.clone()), track(2, same)],
            )
            .unwrap_err(),
            ModelError::BadOrder
        );
        assert_eq!(
            Project::try_assemble(
                Meter::four_four(),
                tempo.clone(),
                Vec::new(),
                vec![track(1, "not-a-key".into())],
            )
            .unwrap_err(),
            ModelError::BadOrder
        );
        let swapped = vec![track(1, sibling_order(1)), track(2, sibling_order(0))];
        assert_eq!(
            Project::try_assemble(
                Meter::four_four(),
                tempo.clone(),
                Vec::new(),
                swapped.clone()
            )
            .unwrap_err(),
            ModelError::BadOrder
        );
        let stored = Project::unchecked(Meter::four_four(), tempo, Vec::new(), swapped);
        assert!(
            stored
                .check_invariants()
                .unwrap_err()
                .to_string()
                .contains("out of arrange order")
        );
    }

    fn fixed_window(start: i64, length: i64, offset: i64) -> Placement {
        Placement {
            id: Id::from_raw(1),
            content: Id::from_raw(2),
            lane: Lane::Timeline,
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(length).unwrap(),
            offset: Tick::quarters(offset).unwrap(),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: DEFAULT_FADE_FLICKS,
            fade_out: DEFAULT_FADE_FLICKS,
        }
    }

    fn tone(start: i64, length: i64) -> Note {
        Note {
            id: Id::from_raw(3),
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(length).unwrap(),
            pitch: 60,
            velocity: 100,
            release: 0,
            articulation: None,
        }
    }

    #[test]
    fn a_window_sounds_only_where_it_overlaps_the_note() {
        let note = tone(0, 4);
        // Content time 0 is the timeline origin when offset equals the window start.
        assert_eq!(fixed_window(2, 2, 2).audible(&tone(0, 1)).unwrap(), None);
        assert_eq!(
            fixed_window(0, 1, 0).audible(&note).unwrap(),
            Some((Tick::new(0), Tick::quarters(1).unwrap()))
        );
        assert_eq!(
            fixed_window(1, 1, 1).audible(&note).unwrap(),
            Some((Tick::quarters(1).unwrap(), Tick::quarters(2).unwrap()))
        );
        assert_eq!(fixed_window(0, 1, 0).audible(&tone(1, 1)).unwrap(), None);
        assert_eq!(
            fixed_window(0, 1, 0).audible(&tone(0, 1)).unwrap(),
            Some((Tick::new(0), Tick::quarters(1).unwrap()))
        );
        assert_eq!(fixed_window(5, 1, 5).audible(&note).unwrap(), None);
        assert!(fixed_window(0, 1, 0).audible(&note).unwrap().is_some());
    }
}
