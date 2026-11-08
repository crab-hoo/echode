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
use std::convert::identity;

use fractional_index::FractionalIndex;

use crate::device::{Device, DeviceKind, MixerIds, valid_plugin_id};
use crate::id::Id;
use crate::note_fields::note_fields;
use crate::time::{FLICKS_PER_SECOND, Flick, Meter, Tempo, TempoMap, Tick};
use crate::track_fields::track_fields;

/// Largest linear clip gain. `2^20` is exact in `f32` and stays finite when a
/// full-scale sample also passes a track fader at `GAIN_MAX`.
pub const CLIP_GAIN_MAX: f32 = 1_048_576.0;

/// Finite clip gain in `0.0..=CLIP_GAIN_MAX`.
pub fn clip_gain_allowed(gain: f32) -> bool {
    gain.is_finite() && (0.0..=CLIP_GAIN_MAX).contains(&gain)
}

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
    InvalidDevice,
    InvalidAudio,
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
            Self::InvalidGain => "gain must be finite, at least zero, and at most 1048576",
            Self::InvalidFade => "fade is negative or does not fit in a frame count",
            Self::Unsorted => "notes are not in start, pitch, id order",
            Self::BadOrder => "track order key is missing, duplicated, or out of arrange order",
            Self::MissingParameter => "parameter is not in the project",
            Self::InvalidParameter => "parameter value is outside its range",
            Self::InvalidDevice => "plugin id is unusable or the device cannot be removed",
            Self::InvalidAudio => "audio reference is not a mono or stereo file",
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

macro_rules! define_note {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $read:ident;)+) => {
        /// A note stored relative to its content.
        ///
        /// `velocity` and `release` are 0..=65535. A MIDI 1.0 value `v` in 0..=127 is
        /// stored as `(v << 9) | (v << 2) | (v >> 5)`, so 127 becomes 65535.
        /// Serde decodes the stored shape; project assembly validates musical values.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Note {
            pub id: Id,
            $(pub $field: $type,)+
        }
    };
}

note_fields!(define_note);

/// Largest MIDI 1.0 pitch or velocity value.
pub const MIDI7_MAX: u8 = 127;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Auto,
    Fixed,
}

/// Where a placement sits. Phase 1 has the timeline only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Timeline,
}

/// Five milliseconds, the default edge fade.
pub const DEFAULT_FADE_FLICKS: i64 = 3_528_000;

/// Sample rate used to reject a fade that cannot become a `u32` frame count.
pub const FADE_RATE_LIMIT: u32 = 768_000;

/// BLAKE3-256 of the decoded samples, plus the facts needed to place them.
///
/// The bytes live in the package store. This value is what the project records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioRef {
    pub hash: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
}

impl AudioRef {
    pub fn try_new(
        hash: String,
        sample_rate: u32,
        channels: u16,
        frames: u64,
    ) -> Result<Self, ModelError> {
        let audio = Self {
            hash,
            sample_rate,
            channels,
            frames,
        };
        audio.check()?;
        Ok(audio)
    }

    pub fn check(&self) -> Result<(), ModelError> {
        if !is_media_hash(&self.hash)
            || self.sample_rate == 0
            || !matches!(self.channels, 1 | 2)
            || self.frames == 0
        {
            return Err(ModelError::InvalidAudio);
        }
        Ok(())
    }
}

pub fn is_media_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

macro_rules! define_content {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $read:ident;)+) => {
        /// Notes and metadata shared by one or more placements.
        /// Content time zero is fixed; notes before it have negative starts.
        #[cfg_attr(not(test), derive(Clone))]
        #[derive(Debug, PartialEq, Eq)]
        pub struct Content {
            pub id: Id,
            $(pub $field: $type,)+
            pub notes: Vec<Note>,
            /// Reserved curves, empty until expression editing is implemented.
            pub expressions: Vec<Expression>,
        }

        // Mirror derived Clone in unit tests to count actual copy-on-write costs.
        #[cfg(test)]
        impl Clone for Content {
            fn clone(&self) -> Self {
                crate::model_scale_tests::content_copy(self.notes.len());
                Self {
                    id: self.id,
                    $($field: self.$field.clone(),)+
                    notes: self.notes.clone(),
                    expressions: self.expressions.clone(),
                }
            }
        }

        /// Package scalars; decoding does not validate references or a project.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct ContentMetadata { $(pub $field: $type,)+ }

        impl From<&Content> for ContentMetadata {
            fn from(content: &Content) -> Self {
                Self { $($field: content.$field.to_owned(),)+ }
            }
        }
        impl ContentMetadata {
            pub fn into_content(self, id: Id, notes: Vec<Note>, expressions: Vec<Expression>) -> Content {
                Content { id, $($field: self.$field,)+ notes, expressions }
            }
        }
    };
}
crate::content_fields::content_fields!(define_content);

macro_rules! define_placement {
    (
        fields {
            Content => $vis:vis content: $content:ty => $state:ty, $value:ident,
                $read:ident, $encode:path, $decode:path; $ffi:ident;
            $($property:ident => $visibility:vis $field:ident: $model:ty => $type:ty,
                $variant:ident, $reader:ident, $encoder:path, $decoder:path; $projection:ident;)+
        }
        diff [$($diff:ident),+];
    ) => {
        /// A window onto shared content. Equality preserves all gain bits.
        /// Serde checks shape only; assembly validates bounds and references.
        #[derive(Clone, Debug, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Placement {
            pub id: Id,
            pub content: $content,
            pub lane: Lane,
            $(pub $field: $model,)+
        }
        impl PartialEq for Placement {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id && self.content == other.content && self.lane == other.lane
                    $(&& $encoder(self.$field) == $encoder(other.$field))+
            }
        }
        impl Eq for Placement {}
    };
}
crate::placement_fields!(define_placement);

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

macro_rules! define_track {
    ($($(#[$attribute:meta])* $property:ident => $field:ident: $type:ty, $value:ident, $read:ident; $ffi:ident $getter:ident;)+) => {
        /// A track. MIDI and audio can share one.
        ///
        /// `parent` is another track, so folders can nest without a depth cap.
        /// Child summing happens later; this records the tree and its contents.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Track {
            pub id: Id,
            $($(#[$attribute])* pub $field: $type,)+
            /// Placements on this track, in timeline order.
            pub clips: Vec<Placement>,
            /// Devices in processing order, ending with one mixer.
            pub devices: Vec<Device>,
        }

        /// Track scalars without identity or children, for package assembly.
        ///
        /// Serde checks shape only. Resolving children and validating the tree,
        /// sibling keys and object identities belongs to `Project::try_assemble`.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct TrackMetadata {
            $($(#[$attribute])* pub $field: $type,)+
        }

        impl From<&Track> for TrackMetadata {
            fn from(track: &Track) -> Self {
                Self { $($field: track.$field.to_owned(),)+ }
            }
        }

        impl TrackMetadata {
            /// Joins decoded scalars to their separately resolved identity and children.
            /// This does not validate a project or repair its order.
            pub fn into_track(self, id: Id, clips: Vec<Placement>, devices: Vec<Device>) -> Track {
                Track { id, $($field: self.$field,)+ clips, devices }
            }
        }
    };
}

track_fields!(define_track);

macro_rules! define_project {
    ($($(#[$attribute:meta])* $property:ident => $field:ident: $type:ty, $value:ident;)+) => {
        /// Immutable project snapshot.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Project {
            $($field: $type,)+
            tempo: TempoMap,
            contents: Vec<Arc<Content>>,
            tracks: Vec<Arc<Track>>,
        }
        /// Project scalars without tempo points or child identities.
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct ProjectMetadata { $($(#[$attribute])* pub $field: $type,)+ }

        impl From<&Project> for ProjectMetadata {
            fn from(project: &Project) -> Self {
                Self { $($field: project.$field,)+ }
            }
        }
        impl ProjectMetadata {
            /// Resolves package children and validates the entire project.
            pub fn try_assemble(self, tempo: TempoMap, contents: Vec<Content>, tracks: Vec<Track>) -> Result<Project, ModelError> {
                Project::try_assemble($(self.$field,)+ tempo, contents, tracks)
            }
        }
    };
}
crate::project_fields::project_fields!(define_project);

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
        content_index(&self.contents, id).map(|index| self.contents[index].as_ref())
    }

    /// The shared content `id`. A caller that keeps the `Arc` can reuse work
    /// derived from it while a later snapshot still holds the same one.
    pub fn shared_content(&self, id: Id) -> Option<&Arc<Content>> {
        content_index(&self.contents, id).map(|index| &self.contents[index])
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
        if !self.track_ids_free(id, mixer) {
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

    /// Adds a track after an anchor's subtree, retaining its sibling group.
    /// When `into_folder` is true and the anchor has children, appends a child
    /// instead. Only the new track receives an order key; existing keys stay put.
    pub fn insert_track_after_with_id(
        &self,
        id: Id,
        after: Id,
        into_folder: bool,
        name: impl Into<String>,
        mixer: MixerIds,
    ) -> Result<Self, ModelError> {
        let anchor = &self.tracks[self.track_index(after)?];
        if into_folder && self.tracks.iter().any(|track| track.parent == Some(after)) {
            return self.add_child_with_id(id, after, name, mixer);
        }
        if !self.track_ids_free(id, mixer) {
            return Err(ModelError::DuplicateId);
        }
        let order = self.order_after(anchor)?;
        let mut tracks = self.tracks.clone();
        tracks.push(Arc::new(Track {
            id,
            name: name.into(),
            parent: anchor.parent,
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

    /// A sibling key between `anchor` and the next sibling under the same parent.
    pub(crate) fn order_after(&self, anchor: &Track) -> Result<String, ModelError> {
        let left = FractionalIndex::from_string(&anchor.order).map_err(|_| ModelError::BadOrder)?;
        let right = self
            .tracks
            .iter()
            .find(|track| track.parent == anchor.parent && track.order > anchor.order);
        let order = match right {
            Some(right) => {
                let right =
                    FractionalIndex::from_string(&right.order).map_err(|_| ModelError::BadOrder)?;
                FractionalIndex::new_between(&left, &right).ok_or(ModelError::BadOrder)?
            }
            None => FractionalIndex::new_after(&left),
        };
        Ok(order.to_string())
    }

    /// Inserts a CLAP device before the mixer. The id is the plugin's CLAP id,
    /// not a file path. Two devices may name the same plugin.
    pub fn add_clap_device(
        &self,
        track_id: Id,
        device_id: Id,
        plugin: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let plugin = plugin.into();
        if !valid_plugin_id(&plugin) {
            return Err(ModelError::InvalidDevice);
        }
        if self.id_in_use(device_id) {
            return Err(ModelError::DuplicateId);
        }
        let index = self.track_index(track_id)?;
        let mut track = (*self.tracks[index]).clone();
        let at = track
            .devices
            .len()
            .checked_sub(1)
            .ok_or(ModelError::InvalidDevice)?;
        track.devices.insert(at, Device::clap(device_id, plugin));
        Ok(self.write_track(index, track))
    }

    /// Records the BLAKE3 of a CLAP device's saved bytes. `None` clears it.
    ///
    /// The bytes themselves live in the package object store. This only stores
    /// the name the journal can restore.
    pub fn set_device_state(
        &self,
        device_id: Id,
        state: Option<String>,
    ) -> Result<Self, ModelError> {
        if state
            .as_deref()
            .is_some_and(|hash| !crate::device::valid_state_hash(hash))
        {
            return Err(ModelError::InvalidDevice);
        }
        let (index, device_index) = self
            .tracks
            .iter()
            .enumerate()
            .find_map(|(index, track)| {
                track
                    .devices
                    .iter()
                    .position(|device| device.id == device_id)
                    .map(|device_index| (index, device_index))
            })
            .ok_or(ModelError::InvalidDevice)?;
        if self.tracks[index].devices[device_index].kind != DeviceKind::Clap {
            return Err(ModelError::InvalidDevice);
        }
        if self.tracks[index].devices[device_index].state == state {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[index]).clone();
        track.devices[device_index].state = state;
        Ok(self.write_track(index, track))
    }

    /// Changes host bypass without changing the device identity or saved state.
    pub fn set_device_bypassed(&self, device: Id, bypassed: bool) -> Result<Self, ModelError> {
        let (owner, index) = self.clap_location(device)?;
        if self.tracks[owner].devices[index].bypassed == bypassed {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[owner]).clone();
        track.devices[index].bypassed = bypassed;
        Ok(self.write_track(owner, track))
    }

    /// Releases the runtime instance without changing identity or recorded settings.
    pub fn set_device_offline(&self, device: Id, offline: bool) -> Result<Self, ModelError> {
        let (owner, index) = self.clap_location(device)?;
        if self.tracks[owner].devices[index].offline == offline {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[owner]).clone();
        track.devices[index].offline = offline;
        Ok(self.write_track(owner, track))
    }

    /// Pins or unpins one CLAP parameter id in a device's list. The id need
    /// not be listed now: a plugin reports its parameters only while running.
    pub fn set_parameter_pinned(
        &self,
        device: Id,
        parameter: u32,
        pinned: bool,
    ) -> Result<Self, ModelError> {
        let (owner, index) = self.clap_location(device)?;
        if parameter == u32::MAX {
            return Err(ModelError::InvalidParameter);
        }
        let current = &self.tracks[owner].devices[index].pinned;
        let at = current.binary_search(&parameter);
        if at.is_ok() == pinned {
            return Ok(self.clone());
        }
        let mut track = (*self.tracks[owner]).clone();
        let list = &mut track.devices[index].pinned;
        match at {
            Ok(at) => {
                list.remove(at);
            }
            Err(at) => list.insert(at, parameter),
        }
        Ok(self.write_track(owner, track))
    }

    /// Reorders a CLAP device inside its track. `None` means just before the
    /// mixer. An id anchor must belong to the same chain; it is never an index
    /// into a snapshot that may have changed while a gesture was pending.
    pub fn reorder_device(&self, device_id: Id, before: Option<Id>) -> Result<Self, ModelError> {
        let (owner, index) = self.clap_location(device_id)?;
        let mut track = (*self.tracks[owner]).clone();
        let anchor = before.unwrap_or_else(|| track.devices.last().expect("track has a mixer").id);
        if !track.devices.iter().any(|device| device.id == anchor) {
            return Err(ModelError::InvalidDevice);
        }
        if anchor == device_id {
            return Ok(self.clone());
        }
        let moved = track.devices.remove(index);
        let at = track
            .devices
            .iter()
            .position(|device| device.id == anchor)
            .ok_or(ModelError::InvalidDevice)?;
        track.devices.insert(at, moved);
        if track.devices == self.tracks[owner].devices {
            return Ok(self.clone());
        }
        Ok(self.write_track(owner, track))
    }

    /// Copies a CLAP device's recorded settings to a new identity. The source
    /// stays, and the immutable saved-state object can be shared. Live instances
    /// remain distinct because the copy has a new device id.
    pub fn copy_device(
        &self,
        source: Id,
        target: Id,
        copy: Id,
        before: Option<Id>,
    ) -> Result<Self, ModelError> {
        let (owner, index) = self.clap_location(source)?;
        if self.id_in_use(copy) {
            return Err(ModelError::DuplicateId);
        }
        let target = self.track_index(target)?;
        let mut device = self.tracks[owner].devices[index].clone();
        device.id = copy;
        let mut track = (*self.tracks[target]).clone();
        let at = match before {
            Some(anchor) => track
                .devices
                .iter()
                .position(|device| device.id == anchor)
                .ok_or(ModelError::InvalidDevice)?,
            None => track
                .devices
                .len()
                .checked_sub(1)
                .ok_or(ModelError::InvalidDevice)?,
        };
        track.devices.insert(at, device);
        Ok(self.write_track(target, track))
    }

    fn clap_location(&self, device_id: Id) -> Result<(usize, usize), ModelError> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(owner, track)| {
                track
                    .devices
                    .iter()
                    .position(|device| device.id == device_id && device.kind == DeviceKind::Clap)
                    .map(|index| (owner, index))
            })
            .ok_or(ModelError::InvalidDevice)
    }

    /// Removes a CLAP device. The mixer stays.
    pub fn remove_device(&self, device_id: Id) -> Result<Self, ModelError> {
        let (index, device_index) = self
            .tracks
            .iter()
            .enumerate()
            .find_map(|(index, track)| {
                track
                    .devices
                    .iter()
                    .position(|device| device.id == device_id)
                    .map(|device_index| (index, device_index))
            })
            .ok_or(ModelError::InvalidDevice)?;
        if self.tracks[index].devices[device_index].kind != DeviceKind::Clap {
            return Err(ModelError::InvalidDevice);
        }
        let mut track = (*self.tracks[index]).clone();
        track.devices.remove(device_index);
        Ok(self.write_track(index, track))
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
        if !self.track_ids_free(id, mixer) {
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
        // Validate the changed forest once instead of searching the whole track
        // list at every ancestor of the destination folder.
        validate_forest(&tracks)?;
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
            },
            &draft,
            bar,
            || clip_id != note_id && note_id != content_id && self.ids_free([clip_id, content_id]),
        )?;
        sort_clips(&mut track.clips);
        tracks[index] = Arc::new(track);
        crate::placement::drop_unreferenced(&mut contents, &tracks);
        Ok(self.with_parts(contents, tracks))
    }

    /// Places a fixed clip whose content is one audio file, with the default
    /// 5 ms edge fades. Notes are not added.
    pub fn place_audio(
        &self,
        track_id: Id,
        clip_id: Id,
        content_id: Id,
        start: Tick,
        audio: AudioRef,
    ) -> Result<Self, ModelError> {
        self.place_audio_with_fades(
            track_id,
            clip_id,
            content_id,
            start,
            audio,
            DEFAULT_FADE_FLICKS,
            DEFAULT_FADE_FLICKS,
        )
    }

    /// Places a fixed clip whose content is one audio file. Fades are in
    /// flicks; zero leaves that edge as recorded. Notes are not added.
    #[allow(clippy::too_many_arguments)]
    pub fn place_audio_with_fades(
        &self,
        track_id: Id,
        clip_id: Id,
        content_id: Id,
        start: Tick,
        audio: AudioRef,
        fade_in: i64,
        fade_out: i64,
    ) -> Result<Self, ModelError> {
        audio.check()?;
        if start.get() < 0 || clip_id == content_id {
            return Err(ModelError::InvalidClip);
        }
        if !fade_allowed(fade_in) || !fade_allowed(fade_out) {
            return Err(ModelError::InvalidClip);
        }
        if !self.ids_free([clip_id, content_id]) {
            return Err(ModelError::DuplicateId);
        }
        let index = self.track_index(track_id)?;
        let length = audio_length(self.tempo(), start, audio.frames, audio.sample_rate)?;
        let mut track = (*self.tracks[index]).clone();
        if ranges_overlap(&track.clips, start.get(), length.get()) {
            return Err(ModelError::Overlap);
        }
        track.clips.push(Placement {
            id: clip_id,
            content: content_id,
            lane: Lane::Timeline,
            start,
            length,
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in,
            fade_out,
        });
        sort_clips(&mut track.clips);
        let mut contents = self.contents.clone();
        contents.push(Arc::new(Content {
            id: content_id,
            audio: Some(audio),
            takes: Vec::new(),
            notes: Vec::new(),
            expressions: Vec::new(),
        }));
        contents.sort_by_key(|content| content.id);
        let mut tracks = self.tracks.clone();
        tracks[index] = Arc::new(track);
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

    /// Copies a note inside its shared content, preserving full-resolution
    /// velocities and expression data. The selected window grows like an edit.
    pub fn duplicate_note(
        &self,
        track: Id,
        clip: Id,
        note: Id,
        copy: Id,
        pitch: u8,
        start: Tick,
    ) -> Result<Self, ModelError> {
        if self.id_in_use(copy) {
            return Err(ModelError::DuplicateId);
        }
        let index = self.track_index(track)?;
        let owner = &self.tracks[index];
        let placement = owner
            .clips
            .iter()
            .find(|item| item.id == clip)
            .ok_or(ModelError::MissingClip)?;
        let content_index =
            content_index(&self.contents, placement.content).ok_or(ModelError::MissingClip)?;
        let mut contents = self.contents.clone();
        let content = &mut contents[content_index];
        let mut copied = content
            .notes
            .iter()
            .find(|item| item.id == note)
            .ok_or(ModelError::MissingNote)?
            .clone();
        copied.id = copy;
        let geometry = NoteGeometry {
            pitch,
            start,
            length: copied.length,
        };
        validate_note(&NoteDraft {
            pitch: geometry.pitch,
            velocity: 0,
            start: geometry.start,
            length: geometry.length,
        })?;
        let expressions: Vec<_> = content
            .expressions
            .iter()
            .filter(|item| item.note == note)
            .cloned()
            .map(|mut item| {
                item.note = copy;
                item
            })
            .collect();
        let content = Arc::make_mut(content);
        content.notes.push(copied);
        content.expressions.extend(expressions);
        let bar = self
            .meter
            .bar_ticks()
            .ok_or(ModelError::InvalidMeter)?
            .get();
        self.edit_note_in_contents(contents, index, clip, copy, geometry, bar)
    }

    /// Edits shared content without replacing the note id or its expression data.
    /// Auto placements refit to bars; time edits grow the edited fixed window
    /// just enough to include the note, leaving other fixed links unchanged.
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
        self.edit_note_in_contents(
            self.contents.clone(),
            index,
            clip_id,
            note_id,
            geometry,
            bar,
        )
    }

    // Consume the caller's content array so a freshly copied note is not cloned
    // again when applying geometry and refitting its shared placements.
    fn edit_note_in_contents(
        &self,
        mut contents: Vec<Arc<Content>>,
        index: usize,
        clip_id: Id,
        note_id: Id,
        geometry: NoteGeometry,
        bar: i64,
    ) -> Result<Self, ModelError> {
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
        let origin = start
            .get()
            .checked_sub(source.offset.get())
            .ok_or(ModelError::InvalidClip)?;
        if start.get().checked_add(source.length.get()).is_none() || source.length.get() <= 0 {
            return Err(ModelError::InvalidClip);
        }
        self.notes_fit(source.content, origin)?;
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

    /// Rejects an origin at which a note of `content` would end past the time range.
    pub(crate) fn notes_fit(&self, content: Id, origin: i64) -> Result<(), ModelError> {
        let Some(content) = self.content(content) else {
            return Ok(());
        };
        for note in &content.notes {
            let absolute = origin
                .checked_add(note.start.get())
                .ok_or(ModelError::InvalidClip)?;
            if note
                .length
                .get()
                .checked_sub(1)
                .and_then(|span| absolute.checked_add(span))
                .is_none()
            {
                return Err(ModelError::InvalidClip);
            }
        }
        Ok(())
    }

    pub fn set_clip_gain(&self, clip_id: Id, gain: f32) -> Result<Self, ModelError> {
        if !clip_gain_allowed(gain) {
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

    pub(crate) fn with_parts(&self, contents: Vec<Arc<Content>>, tracks: Vec<Arc<Track>>) -> Self {
        Self {
            meter: self.meter,
            tempo: self.tempo.clone(),
            contents,
            tracks,
        }
    }

    pub(crate) fn track_index(&self, id: Id) -> Result<usize, ModelError> {
        self.tracks
            .iter()
            .position(|track| track.id == id)
            .ok_or(ModelError::MissingTrack)
    }

    fn track_ids_free(&self, id: Id, mixer: MixerIds) -> bool {
        self.ids_free([
            id,
            mixer.device,
            mixer.gain,
            mixer.pan,
            mixer.mute,
            mixer.solo,
        ])
    }

    fn ids_free<const N: usize>(&self, mut candidates: [Id; N]) -> bool {
        // Keep only the incoming IDs, not every ID in the project.
        candidates.sort_unstable();
        if candidates.windows(2).any(|pair| pair[0] == pair[1]) {
            return false;
        }
        !self.any_id_matches(|candidate| candidates.binary_search(&candidate).is_ok())
    }

    fn id_in_use(&self, id: Id) -> bool {
        self.any_id_matches(|candidate| candidate == id)
    }

    fn any_id_matches(&self, matches: impl Fn(Id) -> bool) -> bool {
        #[cfg(test)]
        crate::model_scale_tests::id_check();
        self.contents.iter().any(|content| {
            matches(content.id)
                || content.takes.iter().copied().any(&matches)
                || content.notes.iter().any(|note| {
                    #[cfg(test)]
                    crate::model_scale_tests::id_check_note();
                    matches(note.id)
                })
        }) || self.tracks.iter().any(|track| {
            matches(track.id)
                || track.clips.iter().any(|clip| matches(clip.id))
                || track.devices.iter().any(|device| {
                    matches(device.id)
                        || device
                            .parameters
                            .iter()
                            .any(|parameter| matches(parameter.id))
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
                #[cfg(test)]
                crate::model_scale_tests::note_validation();
                if !seen.insert(note.id) {
                    return Err(ModelError::DuplicateId);
                }
                if note.pitch > MIDI7_MAX
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
                if !clip_gain_allowed(clip.gain) {
                    return Err(ModelError::InvalidGain);
                }
                if content_index(&stored, clip.content).is_none() {
                    return Err(ModelError::MissingClip);
                }
            }
            if track.clips.windows(2).any(|pair| {
                #[cfg(test)]
                crate::model_scale_tests::overlap();
                // Scalar bounds were checked above. Unsorted non-overlapping
                // input is still rejected by check_invariants below.
                spans_overlap(span(&pair[0]).unwrap(), span(&pair[1]).unwrap())
            }) {
                return Err(ModelError::Overlap);
            }
            crate::device::validate_devices(&track.devices, &mut seen)?;
            assembled.push(Arc::new(track));
        }
        validate_forest(&assembled)?;
        if arrange_fault(&assembled).is_some() {
            return Err(ModelError::BadOrder);
        }
        let referenced = referenced_contents(&assembled);
        if stored
            .iter()
            .any(|content| !referenced.contains(&content.id))
        {
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

/// Contents are kept in ID order at every snapshot boundary.
pub(crate) fn content_index(contents: &[Arc<Content>], id: Id) -> Option<usize> {
    contents
        .binary_search_by_key(&id, |content| {
            #[cfg(test)]
            crate::model_scale_tests::content_key();
            content.id
        })
        .ok()
}

/// Build membership once, rather than scanning all placements per content.
pub(crate) fn referenced_contents(tracks: &[Arc<Track>]) -> HashSet<Id> {
    tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| {
            #[cfg(test)]
            crate::model_scale_tests::reference();
            clip.content
        })
        .collect()
}

fn validate_forest(tracks: &[Arc<Track>]) -> Result<(), ModelError> {
    crate::hierarchy::check_forest(tracks).map_err(|error| match error {
        crate::hierarchy::ForestError::MissingParent { .. } => ModelError::MissingTrack,
        crate::hierarchy::ForestError::Cycle { .. } => ModelError::Cycle,
    })
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
    if draft.pitch > MIDI7_MAX || draft.velocity > MIDI7_MAX || draft.length.get() <= 0 {
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
        #[cfg(test)]
        crate::model_scale_tests::overlap();
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

pub(crate) fn occupied_bars(start: i64, length: i64, bar: i64) -> Option<(i64, i64)> {
    if length <= 0 || bar <= 0 {
        return None;
    }
    let last = length
        .checked_sub(1)
        .and_then(|span| start.checked_add(span))?;
    Some((start.div_euclid(bar), last.div_euclid(bar)))
}

pub(crate) fn bars_touch(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 <= right.1.saturating_add(1) && right.0 <= left.1.saturating_add(1)
}

fn audio_length(
    tempo: &TempoMap,
    start: Tick,
    frames: u64,
    sample_rate: u32,
) -> Result<Tick, ModelError> {
    let start_flick = tempo.flicks_at(start).ok_or(ModelError::InvalidClip)?;
    let duration = i128::from(frames)
        .checked_mul(i128::from(FLICKS_PER_SECOND))
        .ok_or(ModelError::InvalidAudio)?
        / i128::from(sample_rate);
    let duration = i64::try_from(duration).map_err(|_| ModelError::InvalidAudio)?;
    if duration <= 0 {
        return Err(ModelError::InvalidAudio);
    }
    let end_flick = start_flick
        .get()
        .checked_add(duration)
        .ok_or(ModelError::InvalidAudio)?;
    let end = tempo
        .ticks_at(Flick::new(end_flick))
        .ok_or(ModelError::InvalidClip)?;
    let length = end
        .get()
        .checked_sub(start.get())
        .ok_or(ModelError::InvalidClip)?;
    if length <= 0 {
        return Err(ModelError::InvalidClip);
    }
    Ok(Tick::new(length))
}

fn ranges_overlap(clips: &[Placement], start: i64, length: i64) -> bool {
    let Some(end) = start.checked_add(length) else {
        return true;
    };
    clips.iter().any(|clip| {
        let clip_end = clip.start.get().saturating_add(clip.length.get());
        start < clip_end && clip.start.get() < end
    })
}

pub(crate) fn sort_clips(clips: &mut [Placement]) {
    clips.sort_by(|left, right| left.start.cmp(&right.start).then(left.id.cmp(&right.id)));
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
