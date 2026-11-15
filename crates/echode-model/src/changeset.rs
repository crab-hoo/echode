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

#[path = "changeset_content_diff.rs"]
mod content_diff;
use content_diff::ChangedContents;

#[path = "changeset_device.rs"]
mod device;

#[path = "changeset_draft.rs"]
mod draft;
use device::{device_change, diff_device_fields};
use draft::Draft;

#[path = "changeset_note.rs"]
mod note;
use note::{diff_note_fields, note_change};

#[path = "changeset_parameter.rs"]
mod parameter;
use parameter::{diff_parameter_fields, parameter_change};

#[path = "changeset_placement.rs"]
mod placement;
use placement::{clip_change, diff_clip_fields};

#[path = "changeset_project.rs"]
mod project;
use project::diff_project_fields;

#[path = "changeset_tempo.rs"]
mod tempo;
use tempo::diff_tempo;

#[path = "changeset_track.rs"]
mod track;
use track::{diff_track_fields, track_change};

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

macro_rules! define_object_kinds {
    ($($kind:ident => $wire:literal,)+) => {
        /// Kind of object a create or delete names.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum ObjectKind { $($kind,)+ }
        impl ObjectKind {
            /// Every kind, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$kind,)+];
            /// The journal name.
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$kind => $wire,)+ }
            }
            pub fn parse(text: &str) -> Option<Self> {
                match text { $($wire => Some(Self::$kind),)+ _ => None }
            }
        }
    };
}
define_object_kinds! {
    Track => "track",
    Clip => "clip",
    Note => "note",
    TempoPoint => "tempo",
    Content => "content",
    Device => "device",
    Parameter => "parameter",
}

/// An ordered list through which one kind owns objects of another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ownership {
    /// The kind whose `order` property lists the children.
    pub owner: ObjectKind,
    pub order: Property,
}

impl Ownership {
    pub const TRACK_CLIPS: Self = Self::new(ObjectKind::Track, Property::ClipOrder);
    pub const CONTENT_NOTES: Self = Self::new(ObjectKind::Content, Property::NoteOrder);
    pub const TRACK_DEVICES: Self = Self::new(ObjectKind::Track, Property::DeviceOrder);
    pub const DEVICE_PARAMETERS: Self = Self::new(ObjectKind::Device, Property::ParamOrder);

    const fn new(owner: ObjectKind, order: Property) -> Self {
        Self { owner, order }
    }
}

impl ObjectKind {
    /// The lists that may hold objects of this kind; empty for top-level kinds.
    ///
    /// An owned object names its owner in [`Property::Owner`], and exactly one
    /// of these lists of that owner must name it back. [`Project::apply_changeset`]
    /// rejects an object that no list reaches, one listed twice, and one listed
    /// by another owner. The ownership regressions are generated from this table.
    pub const fn owners(self) -> &'static [Ownership] {
        match self {
            Self::TempoPoint | Self::Track | Self::Content => &[],
            Self::Clip => &[Ownership::TRACK_CLIPS],
            Self::Note => &[Ownership::CONTENT_NOTES],
            Self::Device => &[Ownership::TRACK_DEVICES],
            Self::Parameter => &[Ownership::DEVICE_PARAMETERS],
        }
    }
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
        let contents = ChangedContents::between(self, after, &mut creates, &mut updates);
        diff_tracks(
            self,
            after,
            &contents,
            &mut creates,
            &mut updates,
            &mut deletes,
        );
        contents.push_deletes(&mut deletes);
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

fn diff_tracks(
    before: &Project,
    after: &Project,
    contents: &ChangedContents<'_>,
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
    diff_notes(&contents.before, &contents.after, creates, updates, deletes);
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
            Some(previous) => diff_device_fields(previous, device, creates, updates, deletes),
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
    before: &[&Content],
    after: &[&Content],
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let before_notes = notes_of(before);
    let after_notes = notes_of(after);
    for content in after {
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
    for content in before {
        for note in &content.notes {
            #[cfg(test)]
            crate::model_scale_tests::diff_note_visit();
            if !after_notes.contains_key(&note.id) {
                deletes.push(note_change(false, content.id, note));
            }
        }
    }
}

fn notes_of<'a>(contents: &[&'a Content]) -> HashMap<Id, (Id, &'a Note)> {
    let mut notes = HashMap::new();
    for content in contents {
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
