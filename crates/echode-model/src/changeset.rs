//! Property-level difference between two project snapshots.
//!
//! [`Project::diff`] compares objects by id. An unchanged track is skipped with
//! [`Arc::ptr_eq`]. [`Project::apply_changeset`] writes those properties and does
//! not run smart-clip rules. MOD-1.

use std::collections::HashMap;
use std::sync::Arc;

use crate::device::{Device, DeviceKind, ParamKey, Parameter};
use crate::id::Id;
use crate::model::{ClipKind, Content, ModelError, Note, Placement, Project, Track};
use crate::time::{Meter, Tempo, TempoMap, TempoPoint, Tick};

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

/// A stored field. The object kind decides which names are legal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    Meter,
    TrackOrder,
    Name,
    Parent,
    Order,
    ClipOrder,
    Start,
    Length,
    Kind,
    NoteOrder,
    Owner,
    Pitch,
    Velocity,
    Release,
    Articulation,
    Tempo,
    Offset,
    Gain,
    FadeIn,
    FadeOut,
    Lane,
    Content,
    Audio,
    Takes,
    Key,
    Minimum,
    Maximum,
    Default,
    Value,
    ParamOrder,
    DeviceKind,
    DeviceOrder,
    Plugin,
    PluginState,
}

/// A property value. The variant has to match the [`Property`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Meter(Meter),
    Tempo(Tempo),
    Tick(Tick),
    Text(String),
    Id(Id),
    Ids(Vec<Id>),
    Parent(Option<Id>),
    Audio(Option<crate::model::AudioRef>),
    Kind(ClipKind),
    Byte(u8),
    U16(u16),
    Gain(u32),
    Bits(u32),
    Flicks(i64),
    /// One id in an ordered list. `present` is whether this side contains it.
    /// A single insert or removal records this instead of both full lists.
    OrderStep {
        index: u32,
        id: Id,
        present: bool,
    },
}

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
        if self.meter() != after.meter() {
            updates.push(Change::Update {
                id: ObjectKey::Project,
                property: Property::Meter,
                before: Value::Meter(self.meter()),
                after: Value::Meter(after.meter()),
            });
        }
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

fn diff_tempo(
    before: &Project,
    after: &Project,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let previous: HashMap<i64, Tempo> = before
        .tempo()
        .points()
        .iter()
        .map(|point| (point.at.get(), point.tempo))
        .collect();
    let next: HashMap<i64, Tempo> = after
        .tempo()
        .points()
        .iter()
        .map(|point| (point.at.get(), point.tempo))
        .collect();
    let mut created: Vec<i64> = next
        .keys()
        .filter(|tick| !previous.contains_key(tick))
        .copied()
        .collect();
    created.sort_unstable();
    for tick in created {
        creates.push(tempo_change(true, Tick::new(tick), next[&tick]));
    }
    let mut revised: Vec<i64> = next
        .iter()
        .filter(|(tick, tempo)| previous.get(tick).is_some_and(|before| before != *tempo))
        .map(|(tick, _)| *tick)
        .collect();
    revised.sort_unstable();
    for tick in revised {
        updates.push(Change::Update {
            id: ObjectKey::Tempo(Tick::new(tick)),
            property: Property::Tempo,
            before: Value::Tempo(previous[&tick]),
            after: Value::Tempo(next[&tick]),
        });
    }
    let mut removed: Vec<i64> = previous
        .keys()
        .filter(|tick| !next.contains_key(tick))
        .copied()
        .collect();
    removed.sort_unstable();
    for tick in removed {
        deletes.push(tempo_change(false, Tick::new(tick), previous[&tick]));
    }
}

fn tempo_change(create: bool, at: Tick, tempo: Tempo) -> Change {
    let fields = vec![Field {
        property: Property::Tempo,
        value: Value::Tempo(tempo),
    }];
    let id = ObjectKey::Tempo(at);
    if create {
        Change::Create {
            kind: ObjectKind::TempoPoint,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::TempoPoint,
            id,
            fields,
        }
    }
}

fn content_same(before: &Project, after: &Project, id: Id) -> bool {
    let left = before.contents().iter().find(|content| content.id == id);
    let right = after.contents().iter().find(|content| content.id == id);
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
            let before_notes: Vec<Id> = previous.notes.iter().map(|note| note.id).collect();
            let after_notes: Vec<Id> = content.notes.iter().map(|note| note.id).collect();
            if before_notes != after_notes {
                updates.push(note_order_change(content.id, &before_notes, &after_notes));
            }
            if previous.audio != content.audio {
                updates.push(update(
                    ObjectKey::Id(content.id),
                    Property::Audio,
                    Value::Audio(previous.audio.clone()),
                    Value::Audio(content.audio.clone()),
                ));
            }
            if previous.takes != content.takes {
                updates.push(update(
                    ObjectKey::Id(content.id),
                    Property::Takes,
                    Value::Ids(previous.takes.clone()),
                    Value::Ids(content.takes.clone()),
                ));
            }
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

fn content_change(create: bool, content: &Content) -> Change {
    let fields = vec![
        Field {
            property: Property::Audio,
            value: Value::Audio(content.audio.clone()),
        },
        Field {
            property: Property::Takes,
            value: Value::Ids(content.takes.clone()),
        },
        Field {
            property: Property::NoteOrder,
            value: Value::Ids(content.notes.iter().map(|note| note.id).collect()),
        },
    ];
    let id = ObjectKey::Id(content.id);
    if create {
        Change::Create {
            kind: ObjectKind::Content,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Content,
            id,
            fields,
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

fn diff_track_fields(before: &Track, after: &Track, updates: &mut Vec<Change>) {
    if before.name != after.name {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Name,
            Value::Text(before.name.clone()),
            Value::Text(after.name.clone()),
        ));
    }
    if before.parent != after.parent {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Parent,
            Value::Parent(before.parent),
            Value::Parent(after.parent),
        ));
    }
    if before.order != after.order {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Order,
            Value::Text(before.order.clone()),
            Value::Text(after.order.clone()),
        ));
    }
    let before_clips = ids_of_clips(before);
    let after_clips = ids_of_clips(after);
    if before_clips != after_clips {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::ClipOrder,
            Value::Ids(before_clips),
            Value::Ids(after_clips),
        ));
    }
    let before_devices = ids_of_devices(before);
    let after_devices = ids_of_devices(after);
    if before_devices != after_devices {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::DeviceOrder,
            Value::Ids(before_devices),
            Value::Ids(after_devices),
        ));
    }
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
    for device in after_devices {
        match before_devices.iter().find(|item| item.id == device.id) {
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
        if after_devices.iter().any(|item| item.id == device.id) {
            continue;
        }
        for parameter in device.parameters.iter().rev() {
            deletes.push(parameter_change(false, device.id, parameter));
        }
        deletes.push(device_change(false, owner, device));
    }
}

fn diff_device_fields(before: &Device, after: &Device, updates: &mut Vec<Change>) {
    if before.state != after.state {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::PluginState,
            Value::Text(before.state.clone().unwrap_or_default()),
            Value::Text(after.state.clone().unwrap_or_default()),
        ));
    }
    if before
        .parameters
        .iter()
        .map(|parameter| parameter.id)
        .collect::<Vec<_>>()
        != after
            .parameters
            .iter()
            .map(|parameter| parameter.id)
            .collect::<Vec<_>>()
    {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::ParamOrder,
            Value::Ids(
                before
                    .parameters
                    .iter()
                    .map(|parameter| parameter.id)
                    .collect(),
            ),
            Value::Ids(
                after
                    .parameters
                    .iter()
                    .map(|parameter| parameter.id)
                    .collect(),
            ),
        ));
    }
    for parameter in &after.parameters {
        if let Some(previous) = before
            .parameters
            .iter()
            .find(|item| item.id == parameter.id)
        {
            diff_parameter_fields(previous, parameter, updates);
        }
    }
}

fn diff_parameter_fields(before: &Parameter, after: &Parameter, updates: &mut Vec<Change>) {
    let pair = [
        (Property::Minimum, before.min, after.min),
        (Property::Maximum, before.max, after.max),
        (Property::Default, before.default, after.default),
        (Property::Value, before.value, after.value),
    ];
    for (property, left, right) in pair {
        if left.to_bits() != right.to_bits() {
            updates.push(update(
                ObjectKey::Id(after.id),
                property,
                Value::Bits(left.to_bits()),
                Value::Bits(right.to_bits()),
            ));
        }
    }
}

fn device_change(create: bool, track: Id, device: &Device) -> Change {
    let mut fields = vec![
        Field {
            property: Property::Owner,
            value: Value::Id(track),
        },
        Field {
            property: Property::DeviceKind,
            value: Value::Text(device.kind.as_str().to_string()),
        },
    ];
    if let Some(plugin) = &device.plugin {
        fields.push(Field {
            property: Property::Plugin,
            value: Value::Text(plugin.clone()),
        });
    }
    if let Some(state) = &device.state {
        fields.push(Field {
            property: Property::PluginState,
            value: Value::Text(state.clone()),
        });
    }
    fields.push(Field {
        property: Property::ParamOrder,
        value: Value::Ids(
            device
                .parameters
                .iter()
                .map(|parameter| parameter.id)
                .collect(),
        ),
    });
    let id = ObjectKey::Id(device.id);
    if create {
        Change::Create {
            kind: ObjectKind::Device,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Device,
            id,
            fields,
        }
    }
}

fn parameter_change(create: bool, device: Id, parameter: &Parameter) -> Change {
    let fields = vec![
        Field {
            property: Property::Owner,
            value: Value::Id(device),
        },
        Field {
            property: Property::Key,
            value: Value::Text(parameter.key.as_str().to_string()),
        },
        Field {
            property: Property::Minimum,
            value: Value::Bits(parameter.min.to_bits()),
        },
        Field {
            property: Property::Maximum,
            value: Value::Bits(parameter.max.to_bits()),
        },
        Field {
            property: Property::Default,
            value: Value::Bits(parameter.default.to_bits()),
        },
        Field {
            property: Property::Value,
            value: Value::Bits(parameter.value.to_bits()),
        },
    ];
    let id = ObjectKey::Id(parameter.id);
    if create {
        Change::Create {
            kind: ObjectKind::Parameter,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Parameter,
            id,
            fields,
        }
    }
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

fn diff_clip_fields(
    before_owner: Id,
    after_owner: Id,
    before: &Placement,
    after: &Placement,
    updates: &mut Vec<Change>,
) {
    if before_owner != after_owner {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Owner,
            Value::Id(before_owner),
            Value::Id(after_owner),
        ));
    }
    if before.start != after.start {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Start,
            Value::Tick(before.start),
            Value::Tick(after.start),
        ));
    }
    if before.length != after.length {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Length,
            Value::Tick(before.length),
            Value::Tick(after.length),
        ));
    }
    if before.kind != after.kind {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Kind,
            Value::Kind(before.kind),
            Value::Kind(after.kind),
        ));
    }
    if before.offset != after.offset {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Offset,
            Value::Tick(before.offset),
            Value::Tick(after.offset),
        ));
    }
    if before.content != after.content {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Content,
            Value::Id(before.content),
            Value::Id(after.content),
        ));
    }
    if before.gain.to_bits() != after.gain.to_bits() {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Gain,
            Value::Gain(before.gain.to_bits()),
            Value::Gain(after.gain.to_bits()),
        ));
    }
    if before.fade_in != after.fade_in {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::FadeIn,
            Value::Flicks(before.fade_in),
            Value::Flicks(after.fade_in),
        ));
    }
    if before.fade_out != after.fade_out {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::FadeOut,
            Value::Flicks(before.fade_out),
            Value::Flicks(after.fade_out),
        ));
    }
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

fn diff_note_fields(
    before_owner: Id,
    after_owner: Id,
    before: &Note,
    after: &Note,
    updates: &mut Vec<Change>,
) {
    if before_owner != after_owner {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Owner,
            Value::Id(before_owner),
            Value::Id(after_owner),
        ));
    }
    if before.start != after.start {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Start,
            Value::Tick(before.start),
            Value::Tick(after.start),
        ));
    }
    if before.length != after.length {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Length,
            Value::Tick(before.length),
            Value::Tick(after.length),
        ));
    }
    if before.pitch != after.pitch {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Pitch,
            Value::Byte(before.pitch),
            Value::Byte(after.pitch),
        ));
    }
    if before.velocity != after.velocity {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Velocity,
            Value::U16(before.velocity),
            Value::U16(after.velocity),
        ));
    }
    if before.release != after.release {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Release,
            Value::U16(before.release),
            Value::U16(after.release),
        ));
    }
    if before.articulation != after.articulation {
        updates.push(update(
            ObjectKey::Id(after.id),
            Property::Articulation,
            Value::Parent(before.articulation),
            Value::Parent(after.articulation),
        ));
    }
}

fn update(id: ObjectKey, property: Property, before: Value, after: Value) -> Change {
    Change::Update {
        id,
        property,
        before,
        after,
    }
}

fn ids_of_clips(track: &Track) -> Vec<Id> {
    track.clips.iter().map(|clip| clip.id).collect()
}

fn ids_of_devices(track: &Track) -> Vec<Id> {
    track.devices.iter().map(|device| device.id).collect()
}

fn track_change(create: bool, track: &Track) -> Change {
    let fields = vec![
        Field {
            property: Property::Name,
            value: Value::Text(track.name.clone()),
        },
        Field {
            property: Property::Parent,
            value: Value::Parent(track.parent),
        },
        Field {
            property: Property::Order,
            value: Value::Text(track.order.clone()),
        },
        Field {
            property: Property::ClipOrder,
            value: Value::Ids(ids_of_clips(track)),
        },
        Field {
            property: Property::DeviceOrder,
            value: Value::Ids(ids_of_devices(track)),
        },
    ];
    let id = ObjectKey::Id(track.id);
    if create {
        Change::Create {
            kind: ObjectKind::Track,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Track,
            id,
            fields,
        }
    }
}

fn clip_change(create: bool, track: Id, clip: &Placement) -> Change {
    let fields = vec![
        Field {
            property: Property::Owner,
            value: Value::Id(track),
        },
        Field {
            property: Property::Content,
            value: Value::Id(clip.content),
        },
        Field {
            property: Property::Start,
            value: Value::Tick(clip.start),
        },
        Field {
            property: Property::Length,
            value: Value::Tick(clip.length),
        },
        Field {
            property: Property::Offset,
            value: Value::Tick(clip.offset),
        },
        Field {
            property: Property::Kind,
            value: Value::Kind(clip.kind),
        },
        Field {
            property: Property::Gain,
            value: Value::Gain(clip.gain.to_bits()),
        },
        Field {
            property: Property::FadeIn,
            value: Value::Flicks(clip.fade_in),
        },
        Field {
            property: Property::FadeOut,
            value: Value::Flicks(clip.fade_out),
        },
        Field {
            property: Property::Lane,
            value: Value::Text("timeline".to_string()),
        },
    ];
    let id = ObjectKey::Id(clip.id);
    if create {
        Change::Create {
            kind: ObjectKind::Clip,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Clip,
            id,
            fields,
        }
    }
}

fn note_change(create: bool, clip: Id, note: &Note) -> Change {
    let fields = vec![
        Field {
            property: Property::Owner,
            value: Value::Id(clip),
        },
        Field {
            property: Property::Start,
            value: Value::Tick(note.start),
        },
        Field {
            property: Property::Length,
            value: Value::Tick(note.length),
        },
        Field {
            property: Property::Pitch,
            value: Value::Byte(note.pitch),
        },
        Field {
            property: Property::Velocity,
            value: Value::U16(note.velocity),
        },
        Field {
            property: Property::Release,
            value: Value::U16(note.release),
        },
        Field {
            property: Property::Articulation,
            value: Value::Parent(note.articulation),
        },
    ];
    let id = ObjectKey::Id(note.id);
    if create {
        Change::Create {
            kind: ObjectKind::Note,
            id,
            fields,
        }
    } else {
        Change::Delete {
            kind: ObjectKind::Note,
            id,
            fields,
        }
    }
}

struct TrackState {
    name: String,
    parent: Option<Id>,
    order: String,
    clip_order: Vec<Id>,
    device_order: Vec<Id>,
}

struct ContentState {
    audio: Option<crate::model::AudioRef>,
    takes: Vec<Id>,
    note_order: Vec<Id>,
}

struct ClipState {
    track: Id,
    content: Id,
    start: Tick,
    length: Tick,
    offset: Tick,
    kind: ClipKind,
    gain_bits: u32,
    fade_in: i64,
    fade_out: i64,
}

struct NoteState {
    clip: Id,
    start: Tick,
    length: Tick,
    pitch: u8,
    velocity: u16,
    release: u16,
    articulation: Option<Id>,
}

struct DeviceState {
    track: Id,
    kind: String,
    plugin: Option<String>,
    state: Option<String>,
    parameters: Vec<Id>,
}

struct ParamState {
    device: Id,
    key: String,
    min_bits: u32,
    max_bits: u32,
    default_bits: u32,
    value_bits: u32,
}

struct Draft {
    meter: Meter,
    tempos: HashMap<i64, Tempo>,
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
            meter: project.meter(),
            tempos: HashMap::new(),
            tracks: HashMap::new(),
            contents: HashMap::new(),
            clips: HashMap::new(),
            notes: HashMap::new(),
            devices: HashMap::new(),
            parameters: HashMap::new(),
        };
        for content in project.contents() {
            draft.contents.insert(
                content.id,
                ContentState {
                    audio: content.audio.clone(),
                    takes: content.takes.clone(),
                    note_order: content.notes.iter().map(|note| note.id).collect(),
                },
            );
            for note in &content.notes {
                draft.notes.insert(
                    note.id,
                    NoteState {
                        clip: content.id,
                        start: note.start,
                        length: note.length,
                        pitch: note.pitch,
                        velocity: note.velocity,
                        release: note.release,
                        articulation: note.articulation,
                    },
                );
            }
        }
        for point in project.tempo().points() {
            draft.tempos.insert(point.at.get(), point.tempo);
        }
        for track in project.tracks() {
            let mut clip_order = Vec::new();
            for clip in &track.clips {
                clip_order.push(clip.id);
                draft.clips.insert(
                    clip.id,
                    ClipState {
                        track: track.id,
                        content: clip.content,
                        start: clip.start,
                        length: clip.length,
                        offset: clip.offset,
                        kind: clip.kind,
                        gain_bits: clip.gain.to_bits(),
                        fade_in: clip.fade_in,
                        fade_out: clip.fade_out,
                    },
                );
            }
            draft.tracks.insert(
                track.id,
                TrackState {
                    name: track.name.clone(),
                    parent: track.parent,
                    order: track.order.clone(),
                    clip_order,
                    device_order: track.devices.iter().map(|device| device.id).collect(),
                },
            );
            for device in &track.devices {
                draft.devices.insert(
                    device.id,
                    DeviceState {
                        track: track.id,
                        kind: device.kind.as_str().to_string(),
                        plugin: device.plugin.clone(),
                        state: device.state.clone(),
                        parameters: device
                            .parameters
                            .iter()
                            .map(|parameter| parameter.id)
                            .collect(),
                    },
                );
                for parameter in &device.parameters {
                    draft.parameters.insert(
                        parameter.id,
                        ParamState {
                            device: device.id,
                            key: parameter.key.as_str().to_string(),
                            min_bits: parameter.min.to_bits(),
                            max_bits: parameter.max.to_bits(),
                            default_bits: parameter.default.to_bits(),
                            value_bits: parameter.value.to_bits(),
                        },
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
                self.tempos.insert(at.get(), field_tempo(fields)?);
                Ok(())
            }
            (ObjectKind::Track, ObjectKey::Id(id)) => {
                if self.tracks.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.tracks.insert(
                    id,
                    TrackState {
                        name: field_text(fields, Property::Name)?,
                        parent: field_parent(fields)?,
                        order: field_text(fields, Property::Order)?,
                        clip_order: field_ids(fields, Property::ClipOrder)?,
                        device_order: field_ids(fields, Property::DeviceOrder)?,
                    },
                );
                Ok(())
            }
            (ObjectKind::Clip, ObjectKey::Id(id)) => {
                if self.clips.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.clips.insert(
                    id,
                    ClipState {
                        track: field_id(fields, Property::Owner)?,
                        content: field_id(fields, Property::Content)?,
                        start: field_tick(fields, Property::Start)?,
                        length: field_tick(fields, Property::Length)?,
                        offset: field_tick(fields, Property::Offset)?,
                        kind: field_kind(fields)?,
                        gain_bits: field_gain(fields)?,
                        fade_in: field_flicks(fields, Property::FadeIn)?,
                        fade_out: field_flicks(fields, Property::FadeOut)?,
                    },
                );
                Ok(())
            }
            (ObjectKind::Content, ObjectKey::Id(id)) => {
                if self.contents.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.contents.insert(
                    id,
                    ContentState {
                        audio: audio_field(fields)?,
                        takes: field_ids(fields, Property::Takes)?,
                        note_order: field_ids(fields, Property::NoteOrder)?,
                    },
                );
                Ok(())
            }
            (ObjectKind::Device, ObjectKey::Id(id)) => {
                if self.devices.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                let state = field_optional_text(fields, Property::PluginState)?;
                if state
                    .as_deref()
                    .is_some_and(|hash| !crate::device::valid_state_hash(hash))
                {
                    return Err(ModelError::InvalidChange);
                }
                self.devices.insert(
                    id,
                    DeviceState {
                        track: field_id(fields, Property::Owner)?,
                        kind: field_text(fields, Property::DeviceKind)?,
                        plugin: field_optional_text(fields, Property::Plugin)?,
                        state,
                        parameters: field_ids(fields, Property::ParamOrder)?,
                    },
                );
                Ok(())
            }
            (ObjectKind::Parameter, ObjectKey::Id(id)) => {
                if self.parameters.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.parameters.insert(
                    id,
                    ParamState {
                        device: field_id(fields, Property::Owner)?,
                        key: field_text(fields, Property::Key)?,
                        min_bits: field_bits(fields, Property::Minimum)?,
                        max_bits: field_bits(fields, Property::Maximum)?,
                        default_bits: field_bits(fields, Property::Default)?,
                        value_bits: field_bits(fields, Property::Value)?,
                    },
                );
                Ok(())
            }
            (ObjectKind::Note, ObjectKey::Id(id)) => {
                if self.notes.contains_key(&id) {
                    return Err(ModelError::InvalidChange);
                }
                self.notes.insert(
                    id,
                    NoteState {
                        clip: field_id(fields, Property::Owner)?,
                        start: field_tick(fields, Property::Start)?,
                        length: field_tick(fields, Property::Length)?,
                        pitch: field_byte(fields, Property::Pitch)?,
                        velocity: field_u16(fields, Property::Velocity)?,
                        release: field_u16(fields, Property::Release)?,
                        articulation: field_optional_id(fields, Property::Articulation)?,
                    },
                );
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
        match (id, property, value) {
            (ObjectKey::Project, Property::Meter, Value::Meter(meter)) => {
                self.meter = *meter;
                Ok(())
            }
            (ObjectKey::Project, Property::TrackOrder, Value::Ids(_)) => Ok(()),
            (ObjectKey::Tempo(at), Property::Tempo, Value::Tempo(tempo)) => {
                if !self.tempos.contains_key(&at.get()) {
                    return Err(ModelError::InvalidChange);
                }
                self.tempos.insert(at.get(), *tempo);
                Ok(())
            }
            (ObjectKey::Id(id), Property::Name, Value::Text(name)) => {
                self.tracks
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .name = name.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::Parent, Value::Parent(parent)) => {
                self.tracks
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .parent = *parent;
                Ok(())
            }
            (ObjectKey::Id(id), Property::ParamOrder, Value::Ids(ids)) => {
                self.devices
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .parameters = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::Value, Value::Bits(bits)) => {
                self.parameters
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .value_bits = *bits;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Minimum, Value::Bits(bits)) => {
                self.parameters
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .min_bits = *bits;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Maximum, Value::Bits(bits)) => {
                self.parameters
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .max_bits = *bits;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Default, Value::Bits(bits)) => {
                self.parameters
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .default_bits = *bits;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Order, Value::Text(order)) => {
                self.tracks
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .order = order.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::ClipOrder, Value::Ids(ids)) => {
                self.tracks
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .clip_order = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::DeviceOrder, Value::Ids(ids)) => {
                self.tracks
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .device_order = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::Plugin, Value::Text(plugin)) => {
                self.devices
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .plugin = Some(plugin.clone());
                Ok(())
            }
            (ObjectKey::Id(id), Property::PluginState, Value::Text(text)) => {
                let state = if text.is_empty() {
                    None
                } else if crate::device::valid_state_hash(text) {
                    Some(text.clone())
                } else {
                    return Err(ModelError::InvalidChange);
                };
                self.devices
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .state = state;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Start, Value::Tick(tick)) => {
                if let Some(clip) = self.clips.get_mut(&id) {
                    clip.start = *tick;
                    return Ok(());
                }
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .start = *tick;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Length, Value::Tick(tick)) => {
                if let Some(clip) = self.clips.get_mut(&id) {
                    clip.length = *tick;
                    return Ok(());
                }
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .length = *tick;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Kind, Value::Kind(kind)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .kind = *kind;
                Ok(())
            }
            (ObjectKey::Id(id), Property::NoteOrder, Value::Ids(ids)) => {
                self.contents
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .note_order = ids.clone();
                Ok(())
            }
            (
                ObjectKey::Id(id),
                Property::NoteOrder,
                Value::OrderStep {
                    index,
                    id: note,
                    present,
                },
            ) => {
                let order = &mut self
                    .contents
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .note_order;
                let index = usize::try_from(*index).map_err(|_| ModelError::InvalidChange)?;
                if *present {
                    if index > order.len() {
                        return Err(ModelError::InvalidChange);
                    }
                    order.insert(index, *note);
                } else if order.get(index) == Some(note) {
                    order.remove(index);
                } else {
                    return Err(ModelError::InvalidChange);
                }
                Ok(())
            }
            (ObjectKey::Id(id), Property::Audio, value) => {
                let audio = match value {
                    Value::Audio(audio) => audio.clone(),
                    // Journals written before audio files stored a null parent.
                    Value::Parent(None) => None,
                    _ => return Err(ModelError::InvalidChange),
                };
                self.contents
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .audio = audio;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Takes, Value::Ids(ids)) => {
                self.contents
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .takes = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(id), Property::Content, Value::Id(content)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .content = *content;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Offset, Value::Tick(tick)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .offset = *tick;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Gain, Value::Gain(bits)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .gain_bits = *bits;
                Ok(())
            }
            (ObjectKey::Id(id), Property::FadeIn, Value::Flicks(flicks)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .fade_in = *flicks;
                Ok(())
            }
            (ObjectKey::Id(id), Property::FadeOut, Value::Flicks(flicks)) => {
                self.clips
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .fade_out = *flicks;
                Ok(())
            }
            (ObjectKey::Id(_id), Property::Lane, Value::Text(_)) => Ok(()),
            (ObjectKey::Id(id), Property::Owner, Value::Id(owner)) => {
                if let Some(clip) = self.clips.get_mut(&id) {
                    clip.track = *owner;
                    return Ok(());
                }
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .clip = *owner;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Pitch, Value::Byte(byte)) => {
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .pitch = *byte;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Velocity, Value::U16(value)) => {
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .velocity = *value;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Release, Value::U16(value)) => {
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .release = *value;
                Ok(())
            }
            (ObjectKey::Id(id), Property::Articulation, Value::Parent(id_value)) => {
                self.notes
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .articulation = *id_value;
                Ok(())
            }
            _ => Err(ModelError::InvalidChange),
        }
    }

    fn finish(self) -> Result<Project, ModelError> {
        let mut points: Vec<TempoPoint> = self
            .tempos
            .iter()
            .map(|(at, tempo)| TempoPoint {
                at: Tick::new(*at),
                tempo: *tempo,
            })
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
                if note.clip != content_id || seen.insert(*note_id, ()).is_some() {
                    return Err(ModelError::InvalidChange);
                }
                used_notes += 1;
                notes.push(Note {
                    id: *note_id,
                    start: note.start,
                    length: note.length,
                    pitch: note.pitch,
                    velocity: note.velocity,
                    release: note.release,
                    articulation: note.articulation,
                });
            }
            contents.push(Arc::new(Content {
                id: content_id,
                audio: content.audio.clone(),
                takes: content.takes.clone(),
                notes,
                expressions: Vec::new(),
            }));
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
                clips.push(Placement {
                    id: *clip_id,
                    content: clip.content,
                    lane: crate::model::Lane::Timeline,
                    start: clip.start,
                    length: clip.length,
                    offset: clip.offset,
                    kind: clip.kind,
                    gain: f32::from_bits(clip.gain_bits),
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                });
            }
            let mut devices = Vec::with_capacity(track.device_order.len());
            for device_id in &track.device_order {
                let Some(device) = self.devices.get(device_id) else {
                    return Err(ModelError::InvalidChange);
                };
                if device.track != *track_id {
                    return Err(ModelError::InvalidChange);
                }
                let kind = DeviceKind::parse(&device.kind).ok_or(ModelError::InvalidChange)?;
                let mut parameters = Vec::with_capacity(device.parameters.len());
                for parameter_id in &device.parameters {
                    let Some(parameter) = self.parameters.get(parameter_id) else {
                        return Err(ModelError::InvalidChange);
                    };
                    if parameter.device != *device_id {
                        return Err(ModelError::InvalidChange);
                    }
                    let key = ParamKey::parse(&parameter.key).ok_or(ModelError::InvalidChange)?;
                    parameters.push(Parameter {
                        id: *parameter_id,
                        key,
                        min: f32::from_bits(parameter.min_bits),
                        max: f32::from_bits(parameter.max_bits),
                        default: f32::from_bits(parameter.default_bits),
                        value: f32::from_bits(parameter.value_bits),
                    });
                }
                devices.push(Device {
                    id: *device_id,
                    kind,
                    plugin: device.plugin.clone(),
                    state: device.state.clone(),
                    parameters,
                });
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
            tracks.push(Arc::new(Track {
                id: *track_id,
                name: track.name.clone(),
                parent: track.parent,
                order: track.order.clone(),
                clips,
                devices,
            }));
        }
        if used_clips != self.clips.len()
            || used_notes != self.notes.len()
            || used_devices != self.devices.len()
        {
            return Err(ModelError::InvalidChange);
        }
        let tracks = crate::model::arrange_tracks(&tracks)?;
        Ok(Project::from_arcs(self.meter, tempo, contents, tracks))
    }
}

fn field_tempo(fields: &[Field]) -> Result<Tempo, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: Property::Tempo,
                value: Value::Tempo(tempo),
            } => Some(*tempo),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
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

fn field_parent(fields: &[Field]) -> Result<Option<Id>, ModelError> {
    field_optional_id(fields, Property::Parent)
}

fn audio_field(fields: &[Field]) -> Result<Option<crate::model::AudioRef>, ModelError> {
    let value = fields
        .iter()
        .find(|field| field.property == Property::Audio)
        .ok_or(ModelError::InvalidChange)?;
    match &value.value {
        Value::Audio(audio) => Ok(audio.clone()),
        Value::Parent(None) => Ok(None),
        _ => Err(ModelError::InvalidChange),
    }
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

fn field_kind(fields: &[Field]) -> Result<ClipKind, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: Property::Kind,
                value: Value::Kind(kind),
            } => Some(*kind),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_gain(fields: &[Field]) -> Result<u32, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: Property::Gain,
                value: Value::Gain(bits),
            } => Some(*bits),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_flicks(fields: &[Field], property: Property) -> Result<i64, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Flicks(flicks),
            } if *found == property => Some(*flicks),
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
