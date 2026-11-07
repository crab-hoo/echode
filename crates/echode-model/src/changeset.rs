//! Property-level difference between two project snapshots.
//!
//! [`Project::diff`] compares objects by id. An unchanged track is skipped with
//! [`Arc::ptr_eq`]. [`Project::apply_changeset`] writes those properties and does
//! not run smart-clip rules. MOD-1.

use std::collections::HashMap;
use std::sync::Arc;

use crate::device::Device;
use crate::id::Id;
use crate::model::{ClipKind, Content, ModelError, Note, Placement, Project, Track};
use crate::time::{Meter, Tempo, TempoMap, TempoPoint, Tick};

#[path = "changeset_content.rs"]
mod content;
use content::{ContentState, content_change, diff_content_fields};

#[path = "changeset_device.rs"]
mod device;
use device::{DeviceState, device_change, diff_device_fields};

#[path = "changeset_note.rs"]
mod note;
use note::{NoteState, diff_note_fields, note_change};

#[path = "changeset_parameter.rs"]
mod parameter;
use parameter::{ParamState, diff_parameter_fields, parameter_change};

#[path = "changeset_placement.rs"]
mod placement;
use placement::{ClipState, clip_change, diff_clip_fields};

#[path = "changeset_project.rs"]
mod project;
use project::{ProjectState, diff_project_fields};

#[path = "changeset_tempo.rs"]
mod tempo;
use tempo::{TempoState, diff_tempo};

#[path = "changeset_track.rs"]
mod track;
use track::{TrackState, diff_track_fields, track_change};

/// One property written by a [`Change`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub property: Property,
    pub value: Value,
}

/// What an edit did to one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Create {
        kind: ObjectKind,
        id: ObjectKey,
        fields: Vec<Field>,
    },
    Delete {
        kind: ObjectKind,
        id: ObjectKey,
        fields: Vec<Field>,
    },
    Update {
        id: ObjectKey,
        property: Property,
        before: Value,
        after: Value,
    },
}

/// Kind of object a create or delete names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Track,
    Clip,
    Note,
    TempoPoint,
    Content,
    Device,
    Parameter,
}

/// Which object a change addresses.
///
/// Track, clip, and note ids share one space. Tempo points are addressed by
/// their tick, and the project itself has the meter and the track order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKey {
    Project,
    Tempo(Tick),
    Id(Id),
}

macro_rules! define_properties {
    ($($property:ident => $wire:literal,)+) => {
        /// A stored field. The object kind decides which names are legal.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Property { $($property,)+ }
        impl Property {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$property => $wire,)+ }
            }
            pub fn parse(text: &str) -> Option<Self> {
                match text { $($wire => Some(Self::$property),)+ _ => None }
            }
        }
    };
}
define_properties! {
    Meter => "meter",
    TrackOrder => "track_order",
    Name => "name",
    Parent => "parent",
    Order => "order",
    ClipOrder => "clip_order",
    Start => "start",
    Length => "length",
    Kind => "kind",
    NoteOrder => "note_order",
    Owner => "owner",
    Pitch => "pitch",
    Velocity => "velocity",
    Release => "release",
    Articulation => "articulation",
    Tempo => "tempo",
    Offset => "offset",
    Gain => "gain",
    FadeIn => "fade_in",
    FadeOut => "fade_out",
    Lane => "lane",
    Content => "content",
    Audio => "audio",
    Takes => "takes",
    Key => "key",
    Minimum => "minimum",
    Maximum => "maximum",
    Default => "default",
    Value => "value",
    ParamOrder => "param_order",
    DeviceKind => "device_kind",
    DeviceOrder => "device_order",
    Plugin => "plugin",
    PluginState => "plugin_state",
    Bypassed => "bypassed",
    Offline => "offline",
    PinnedParameters => "pinned_parameters",
}

pub use crate::change_values::Value;

/// Ordered property changes. Forward application reaches the later snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    pub fn new() -> Self {
        Self {
            changes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn append(&mut self, other: Self) {
        self.changes.extend(other.changes);
    }

    pub fn push(&mut self, change: Change) {
        self.changes.push(change);
    }

    /// The reverse list. Applying it to the later snapshot restores the earlier one.
    pub fn invert(&self) -> Self {
        Self {
            changes: self
                .changes
                .iter()
                .rev()
                .map(|change| change.invert())
                .collect(),
        }
    }
}

impl Default for ChangeSet {
    fn default() -> Self {
        Self::new()
    }
}

impl Change {
    fn invert(&self) -> Self {
        match self {
            Self::Create { kind, id, fields } => Self::Delete {
                kind: *kind,
                id: *id,
                fields: fields.clone(),
            },
            Self::Delete { kind, id, fields } => Self::Create {
                kind: *kind,
                id: *id,
                fields: fields.clone(),
            },
            Self::Update {
                id,
                property,
                before,
                after,
            } => Self::Update {
                id: *id,
                property: *property,
                before: after.clone(),
                after: before.clone(),
            },
        }
    }
}

impl Project {
    /// Properties that differ. Unchanged tracks are not walked.
    pub fn diff(&self, after: &Project) -> ChangeSet {
        let mut creates = Vec::new();
        let mut updates = Vec::new();
        let mut deletes = Vec::new();
        diff_tempo(self, after, &mut creates, &mut updates, &mut deletes);
        diff_project_fields(self, after, &mut updates);
        diff_content_shells(self, after, &mut creates, &mut updates);
        diff_tracks(self, after, &mut creates, &mut updates, &mut deletes);
        push_content_deletes(self, after, &mut deletes);
        creates.extend(updates);
        creates.extend(deletes);
        ChangeSet { changes: creates }
    }

    /// Writes `changes` onto this snapshot without refitting clips.
    pub fn apply_changeset(&self, changes: &ChangeSet) -> Result<Self, ModelError> {
        let mut draft = Draft::from_project(self);
        for change in &changes.changes {
            draft.apply(change)?;
        }
        draft.finish()
    }
}

fn content_same(before: &Project, after: &Project, id: Id) -> bool {
    let left = before.shared_content(id);
    let right = after.shared_content(id);
    matches!((left, right), (Some(left), Some(right)) if Arc::ptr_eq(left, right))
}

fn diff_content_shells(
    before: &Project,
    after: &Project,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
) {
    for content in after.contents() {
        if let Some(previous) = before.content(content.id) {
            if content_same(before, after, content.id) {
                continue;
            }
            diff_content_fields(previous, content, updates);
        } else {
            creates.push(content_change(true, content));
        }
    }
}

fn push_content_deletes(before: &Project, after: &Project, deletes: &mut Vec<Change>) {
    for content in before.contents() {
        if after.content(content.id).is_none() {
            deletes.push(content_change(false, content));
        }
    }
}

fn diff_tracks(
    before: &Project,
    after: &Project,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let before_by_id = tracks_by_id(before);
    let after_by_id = tracks_by_id(after);
    for track in after.tracks() {
        match before_by_id.get(&track.id) {
            None => {
                creates.push(track_change(true, track));
                diff_devices(None, Some(track), creates, updates, deletes);
            }
            Some(previous) if Arc::ptr_eq(*previous, track) => {}
            Some(previous) => {
                diff_track_fields(previous, track, updates);
                diff_devices(Some(previous), Some(track), creates, updates, deletes);
            }
        }
    }
    diff_clips(
        before,
        after,
        &before_by_id,
        &after_by_id,
        creates,
        updates,
        deletes,
    );
    diff_notes(
        before,
        after,
        &before_by_id,
        &after_by_id,
        creates,
        updates,
        deletes,
    );
    for track in before.tracks() {
        if !after_by_id.contains_key(&track.id) {
            diff_devices(Some(track), None, creates, updates, deletes);
            deletes.push(track_change(false, track));
        }
    }
}

fn tracks_by_id(project: &Project) -> HashMap<Id, &Arc<Track>> {
    project
        .tracks()
        .iter()
        .map(|track| (track.id, track))
        .collect()
}

fn unchanged(before: &HashMap<Id, &Arc<Track>>, after: &HashMap<Id, &Arc<Track>>, id: Id) -> bool {
    matches!((before.get(&id), after.get(&id)), (Some(left), Some(right)) if Arc::ptr_eq(*left, *right))
}

fn diff_devices(
    before: Option<&Track>,
    after: Option<&Track>,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let empty: &[Device] = &[];
    let before_devices = before
        .map(|track| track.devices.as_slice())
        .unwrap_or(empty);
    let after_devices = after.map(|track| track.devices.as_slice()).unwrap_or(empty);
    let owner = after.or(before).map(|track| track.id);
    let Some(owner) = owner else {
        return;
    };
    // Consume matches, but emit changes in chain order rather than hash order.
    let mut unmatched: HashMap<Id, &Device> = before_devices
        .iter()
        .map(|device| (diff_device_key(device), device))
        .collect();
    for device in after_devices {
        match unmatched.remove(&diff_device_key(device)) {
            None => {
                creates.push(device_change(true, owner, device));
                for parameter in &device.parameters {
                    creates.push(parameter_change(true, device.id, parameter));
                }
            }
            Some(previous) => diff_device_fields(previous, device, updates),
        }
    }
    for device in before_devices {
        if !unmatched.contains_key(&diff_device_key(device)) {
            continue;
        }
        for parameter in device.parameters.iter().rev() {
            deletes.push(parameter_change(false, device.id, parameter));
        }
        deletes.push(device_change(false, owner, device));
    }
}

fn diff_device_key(device: &Device) -> Id {
    #[cfg(test)]
    crate::model_scale_tests::diff_device_key();
    device.id
}

fn diff_clips(
    before: &Project,
    after: &Project,
    before_tracks: &HashMap<Id, &Arc<Track>>,
    after_tracks: &HashMap<Id, &Arc<Track>>,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let before_clips = clips_outside_unchanged(before, before_tracks, after_tracks);
    let after_clips = clips_outside_unchanged(after, after_tracks, before_tracks);
    for track in after.tracks() {
        if unchanged(before_tracks, after_tracks, track.id) {
            continue;
        }
        for clip in &track.clips {
            if let Some((owner, previous)) = before_clips.get(&clip.id) {
                diff_clip_fields(*owner, track.id, previous, clip, updates);
            } else {
                creates.push(clip_change(true, track.id, clip));
            }
        }
    }
    for track in before.tracks() {
        if unchanged(before_tracks, after_tracks, track.id) {
            continue;
        }
        for clip in &track.clips {
            if !after_clips.contains_key(&clip.id) {
                deletes.push(clip_change(false, track.id, clip));
            }
        }
    }
}

fn clips_outside_unchanged<'a>(
    project: &'a Project,
    own: &HashMap<Id, &Arc<Track>>,
    other: &HashMap<Id, &Arc<Track>>,
) -> HashMap<Id, (Id, &'a Placement)> {
    let mut clips = HashMap::new();
    for track in project.tracks() {
        if unchanged(own, other, track.id) {
            continue;
        }
        for clip in &track.clips {
            clips.insert(clip.id, (track.id, clip));
        }
    }
    clips
}

fn diff_notes(
    before: &Project,
    after: &Project,
    before_tracks: &HashMap<Id, &Arc<Track>>,
    after_tracks: &HashMap<Id, &Arc<Track>>,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let before_notes = notes_of(before, after);
    let after_notes = notes_of(after, before);
    let _ = (before_tracks, after_tracks);
    for content in after.contents() {
        if content_same(before, after, content.id) {
            continue;
        }
        for note in &content.notes {
            #[cfg(test)]
            crate::model_scale_tests::diff_note_visit();
            if let Some((owner, previous)) = before_notes.get(&note.id) {
                diff_note_fields(*owner, content.id, previous, note, updates);
            } else {
                creates.push(note_change(true, content.id, note));
            }
        }
    }
    for content in before.contents() {
        if content_same(before, after, content.id) {
            continue;
        }
        for note in &content.notes {
            #[cfg(test)]
            crate::model_scale_tests::diff_note_visit();
            if !after_notes.contains_key(&note.id) {
                deletes.push(note_change(false, content.id, note));
            }
        }
    }
}

fn notes_of<'a>(project: &'a Project, other: &Project) -> HashMap<Id, (Id, &'a Note)> {
    let mut notes = HashMap::new();
    for content in project.contents() {
        if content_same(project, other, content.id) {
            continue;
        }
        for note in &content.notes {
            #[cfg(test)]
            crate::model_scale_tests::diff_note_visit();
            notes.insert(note.id, (content.id, note));
        }
    }
    notes
}

/// A one-id insert or removal stays a step. Any other reorder keeps both lists,
/// which is what older journals already store.
fn note_order_change(content: Id, before: &[Id], after: &[Id]) -> Change {
    let (before_value, after_value) = if let Some((index, id)) = one_inserted(before, after) {
        (
            Value::OrderStep {
                index,
                id,
                present: false,
            },
            Value::OrderStep {
                index,
                id,
                present: true,
            },
        )
    } else if let Some((index, id)) = one_inserted(after, before) {
        (
            Value::OrderStep {
                index,
                id,
                present: true,
            },
            Value::OrderStep {
                index,
                id,
                present: false,
            },
        )
    } else {
        (Value::Ids(before.to_vec()), Value::Ids(after.to_vec()))
    };
    update(
        ObjectKey::Id(content),
        Property::NoteOrder,
        before_value,
        after_value,
    )
}

fn one_inserted(before: &[Id], after: &[Id]) -> Option<(u32, Id)> {
    if after.len() != before.len().saturating_add(1) {
        return None;
    }
    let mut index = 0;
    while index < before.len() && before[index] == after[index] {
        index += 1;
    }
    let id = *after.get(index)?;
    if after.get(index + 1..)? != before.get(index..)? {
        return None;
    }
    u32::try_from(index).ok().map(|index| (index, id))
}

fn update(id: ObjectKey, property: Property, before: Value, after: Value) -> Change {
    Change::Update {
        id,
        property,
        before,
        after,
    }
}

struct Draft {
    project: ProjectState,
    tempos: HashMap<i64, TempoState>,
    tracks: HashMap<Id, TrackState>,
    contents: HashMap<Id, ContentState>,
    clips: HashMap<Id, ClipState>,
    notes: HashMap<Id, NoteState>,
    devices: HashMap<Id, DeviceState>,
    parameters: HashMap<Id, ParamState>,
}

impl Draft {
    fn from_project(project: &Project) -> Self {
        let mut draft = Self {
            project: ProjectState::from_project(project),
            tempos: HashMap::new(),
            tracks: HashMap::new(),
            contents: HashMap::new(),
            clips: HashMap::new(),
            notes: HashMap::new(),
            devices: HashMap::new(),
            parameters: HashMap::new(),
        };
        for content in project.contents() {
            draft
                .contents
                .insert(content.id, ContentState::from_content(content));
            for note in &content.notes {
                draft
                    .notes
                    .insert(note.id, NoteState::from_note(content.id, note));
            }
        }
        for point in project.tempo().points() {
            draft
                .tempos
                .insert(point.at.get(), TempoState::from_point(point));
        }
        for track in project.tracks() {
            for clip in &track.clips {
                draft
                    .clips
                    .insert(clip.id, ClipState::from_clip(track.id, clip));
            }
            draft.tracks.insert(track.id, TrackState::from_track(track));
            for device in &track.devices {
                draft
                    .devices
                    .insert(device.id, DeviceState::from_device(track.id, device));
                for parameter in &device.parameters {
                    draft.parameters.insert(
                        parameter.id,
                        ParamState::from_parameter(device.id, parameter),
                    );
                }
            }
        }
        draft
    }

    fn apply(&mut self, change: &Change) -> Result<(), ModelError> {
        match change {
            Change::Create { kind, id, fields } => self.insert(*kind, *id, fields),
            Change::Delete { kind, id, .. } => self.remove(*kind, *id),
            Change::Update {
                id,
                property,
                after,
                ..
            } => self.set(*id, *property, after),
        }
    }

    fn insert(
        &mut self,
        kind: ObjectKind,
        id: ObjectKey,
        fields: &[Field],
    ) -> Result<(), ModelError> {
        match (kind, id) {
            (ObjectKind::TempoPoint, ObjectKey::Tempo(at)) => {
                if self.tempos.contains_key(&at.get()) {
                    return Err(ModelError::InvalidChange);
                }
                self.tempos
                    .insert(at.get(), TempoState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Track, ObjectKey::Id(id)) => {
                if self.tracks.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.tracks.insert(id, TrackState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Clip, ObjectKey::Id(id)) => {
                if self.clips.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.clips.insert(id, ClipState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Content, ObjectKey::Id(id)) => {
                if self.contents.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.contents.insert(id, ContentState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Device, ObjectKey::Id(id)) => {
                if self.devices.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.devices.insert(id, DeviceState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Parameter, ObjectKey::Id(id)) => {
                if self.parameters.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.parameters.insert(id, ParamState::from_fields(fields)?);
                Ok(())
            }
            (ObjectKind::Note, ObjectKey::Id(id)) => {
                if self.notes.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.notes.insert(id, NoteState::from_fields(fields)?);
                Ok(())
            }
            _ => Err(ModelError::InvalidChange),
        }
    }

    fn remove(&mut self, kind: ObjectKind, id: ObjectKey) -> Result<(), ModelError> {
        let removed = match (kind, id) {
            (ObjectKind::TempoPoint, ObjectKey::Tempo(at)) => {
                self.tempos.remove(&at.get()).is_some()
            }
            (ObjectKind::Track, ObjectKey::Id(id)) => self.tracks.remove(&id).is_some(),
            (ObjectKind::Clip, ObjectKey::Id(id)) => self.clips.remove(&id).is_some(),
            (ObjectKind::Note, ObjectKey::Id(id)) => self.notes.remove(&id).is_some(),
            (ObjectKind::Content, ObjectKey::Id(id)) => self.contents.remove(&id).is_some(),
            (ObjectKind::Device, ObjectKey::Id(id)) => self.devices.remove(&id).is_some(),
            (ObjectKind::Parameter, ObjectKey::Id(id)) => self.parameters.remove(&id).is_some(),
            _ => false,
        };
        if removed {
            Ok(())
        } else {
            Err(ModelError::InvalidChange)
        }
    }

    fn set(&mut self, id: ObjectKey, property: Property, value: &Value) -> Result<(), ModelError> {
        if let ObjectKey::Id(id) = id
            && let Some(parameter) = self.parameters.get_mut(&id)
        {
            return parameter.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(note) = self.notes.get_mut(&id)
        {
            return note.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && DeviceState::is_scalar(property)
        {
            return self
                .devices
                .get_mut(&id)
                .ok_or(ModelError::InvalidChange)?
                .set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(track) = self.tracks.get_mut(&id)
        {
            return track.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(content) = self.contents.get_mut(&id)
        {
            return content.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(clip) = self.clips.get_mut(&id)
        {
            return clip.set(property, value);
        }
        match (id, property, value) {
            (ObjectKey::Project, property, value) => self.project.set(property, value),
            (ObjectKey::Tempo(at), property, value) => self
                .tempos
                .get_mut(&at.get())
                .ok_or(ModelError::InvalidChange)?
                .set(property, value),
            (ObjectKey::Id(id), Property::ParamOrder, Value::Ids(ids)) => {
                self.devices
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .parameters = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(_id), Property::Lane, Value::Text(_)) => Ok(()),
            _ => Err(ModelError::InvalidChange),
        }
    }

    fn finish(self) -> Result<Project, ModelError> {
        let mut points: Vec<TempoPoint> = self
            .tempos
            .iter()
            .map(|(at, point)| point.finish(Tick::new(*at)))
            .collect();
        points.sort_by_key(|point| point.at);
        let tempo = TempoMap::try_from_points(points).ok_or(ModelError::InvalidChange)?;
        let mut seen = HashMap::<Id, ()>::new();
        let mut contents = Vec::new();
        let mut content_ids: Vec<Id> = self.contents.keys().copied().collect();
        content_ids.sort();
        let mut used_notes = 0;
        for content_id in content_ids {
            if seen.insert(content_id, ()).is_some() {
                return Err(ModelError::DuplicateId);
            }
            let content = &self.contents[&content_id];
            let mut notes = Vec::with_capacity(content.note_order.len());
            for note_id in &content.note_order {
                let Some(note) = self.notes.get(note_id) else {
                    return Err(ModelError::InvalidChange);
                };
                if note.content != content_id || seen.insert(*note_id, ()).is_some() {
                    return Err(ModelError::InvalidChange);
                }
                used_notes += 1;
                notes.push(note.finish(*note_id));
            }
            contents.push(Arc::new(content.finish(content_id, notes)));
        }
        let mut tracks = Vec::with_capacity(self.tracks.len());
        let mut used_clips = 0;
        let mut used_devices = 0;
        for (track_id, track) in &self.tracks {
            if seen.insert(*track_id, ()).is_some() {
                return Err(ModelError::DuplicateId);
            }
            let mut clips = Vec::with_capacity(track.clip_order.len());
            for clip_id in &track.clip_order {
                let Some(clip) = self.clips.get(clip_id) else {
                    return Err(ModelError::InvalidChange);
                };
                if clip.track != *track_id || seen.insert(*clip_id, ()).is_some() {
                    return Err(ModelError::InvalidChange);
                }
                if !self.contents.contains_key(&clip.content) {
                    return Err(ModelError::InvalidChange);
                }
                used_clips += 1;
                clips.push(clip.finish(*clip_id));
            }
            let mut devices = Vec::with_capacity(track.device_order.len());
            for device_id in &track.device_order {
                let Some(device) = self.devices.get(device_id) else {
                    return Err(ModelError::InvalidChange);
                };
                if device.track != *track_id {
                    return Err(ModelError::InvalidChange);
                }
                let mut parameters = Vec::with_capacity(device.parameters.len());
                for parameter_id in &device.parameters {
                    let Some(parameter) = self.parameters.get(parameter_id) else {
                        return Err(ModelError::InvalidChange);
                    };
                    if parameter.device != *device_id {
                        return Err(ModelError::InvalidChange);
                    }
                    parameters.push(parameter.finish(*parameter_id)?);
                }
                devices.push(device.finish(*device_id, parameters)?);
            }
            let owned = self
                .devices
                .values()
                .filter(|device| device.track == *track_id)
                .count();
            if owned != devices.len() {
                return Err(ModelError::InvalidChange);
            }
            used_devices += devices.len();
            tracks.push(Arc::new(track.finish(*track_id, clips, devices)));
        }
        if used_clips != self.clips.len()
            || used_notes != self.notes.len()
            || used_devices != self.devices.len()
        {
            return Err(ModelError::InvalidChange);
        }
        let tracks = crate::model::arrange_tracks(&tracks)?;
        Ok(self.project.finish(tempo, contents, tracks))
    }
}

fn field_bits(fields: &[Field], property: Property) -> Result<u32, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Bits(bits),
            } if *found == property => Some(*bits),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_optional_text(fields: &[Field], property: Property) -> Result<Option<String>, ModelError> {
    let mut found = None;
    for field in fields {
        if field.property != property {
            continue;
        }
        if found.is_some() {
            return Err(ModelError::InvalidChange);
        }
        match &field.value {
            Value::Text(text) => found = Some(text.clone()),
            _ => return Err(ModelError::InvalidChange),
        }
    }
    Ok(found)
}

fn field_text(fields: &[Field], property: Property) -> Result<String, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Text(text),
            } if *found == property => Some(text.clone()),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_optional_id(fields: &[Field], property: Property) -> Result<Option<Id>, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Parent(parent),
            } if *found == property => Some(*parent),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_ids(fields: &[Field], property: Property) -> Result<Vec<Id>, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Ids(ids),
            } if *found == property => Some(ids.clone()),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_id(fields: &[Field], property: Property) -> Result<Id, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Id(id),
            } if *found == property => Some(*id),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_tick(fields: &[Field], property: Property) -> Result<Tick, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Tick(tick),
            } if *found == property => Some(*tick),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_u16(fields: &[Field], property: Property) -> Result<u16, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::U16(value),
            } if *found == property => Some(*value),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_byte(fields: &[Field], property: Property) -> Result<u8, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Byte(byte),
            } if *found == property => Some(*byte),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

#[cfg(test)]
#[path = "changeset_tests.rs"]
mod tests;
