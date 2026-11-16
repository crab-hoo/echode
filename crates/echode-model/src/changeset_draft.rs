//! Replay target for [`Project::apply_changeset`].
//!
//! Objects are stored by kind. Top-level kinds are taken whole; every other
//! object must be named by exactly one ordered list of the owner it names, as
//! declared by [`ObjectKind::owners`]. A changeset that leaves an object outside
//! those lists is rejected instead of silently dropping it.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

use super::content::ContentState;
use super::device::DeviceState;
use super::note::NoteState;
use super::parameter::ParamState;
use super::placement::ClipState;
use super::project::ProjectState;
use super::route::{SendState, SidechainState};
use super::tempo::TempoState;
use super::track::TrackState;
use super::{Change, Field, Id, ModelError, ObjectKey, ObjectKind, Ownership, Property, Value};
use crate::model::Project;
use crate::time::{TempoMap, TempoPoint, Tick};

/// A draft object that exists only inside an owner's ordered list.
trait Child {
    const KIND: ObjectKind;
    /// The id stored in `Property::Owner`.
    fn owner(&self) -> Id;
}

impl Child for ClipState {
    const KIND: ObjectKind = ObjectKind::Clip;
    fn owner(&self) -> Id {
        self.track
    }
}

impl Child for NoteState {
    const KIND: ObjectKind = ObjectKind::Note;
    fn owner(&self) -> Id {
        self.content
    }
}

impl Child for DeviceState {
    const KIND: ObjectKind = ObjectKind::Device;
    fn owner(&self) -> Id {
        self.track
    }
}

impl Child for ParamState {
    const KIND: ObjectKind = ObjectKind::Parameter;
    fn owner(&self) -> Id {
        self.device
    }
}

impl Child for SendState {
    const KIND: ObjectKind = ObjectKind::Send;
    fn owner(&self) -> Id {
        self.track
    }
}

impl Child for SidechainState {
    const KIND: ObjectKind = ObjectKind::Sidechain;
    fn owner(&self) -> Id {
        self.device
    }
}

/// Draft objects of one owned kind, with how many owner lists have reached.
struct Children<S> {
    items: HashMap<Id, S>,
    listed: usize,
}

impl<S: Child> Children<S> {
    fn new() -> Self {
        Self {
            items: HashMap::new(),
            listed: 0,
        }
    }

    fn insert(&mut self, id: Id, state: S) -> Result<(), ModelError> {
        insert_new(&mut self.items, id, state)
    }

    fn remove(&mut self, id: Id) -> Result<(), ModelError> {
        remove_existing(&mut self.items, id)
    }

    fn get_mut(&mut self, id: &Id) -> Option<&mut S> {
        self.items.get_mut(id)
    }

    /// Builds `owner`'s children in list order. Each id must name a stored
    /// child of `owner` and appear once among all objects and lists; `build`
    /// gets `seen` back to resolve the child's own lists.
    fn resolve<T>(
        &mut self,
        list: Ownership,
        owner: Id,
        order: &[Id],
        seen: &mut HashSet<Id>,
        mut build: impl FnMut(Id, &S, &mut HashSet<Id>) -> Result<T, ModelError>,
    ) -> Result<Vec<T>, ModelError> {
        // Ownership regressions are generated from the table; keep it complete.
        debug_assert!(
            S::KIND.owners().contains(&list),
            "{:?} is resolved from {list:?}, which ObjectKind::owners does not declare",
            S::KIND
        );
        let mut built = Vec::with_capacity(order.len());
        for id in order {
            let child = self.items.get(id).ok_or(ModelError::InvalidChange)?;
            if child.owner() != owner || !seen.insert(*id) {
                return Err(ModelError::InvalidChange);
            }
            self.listed += 1;
            built.push(build(*id, child, seen)?);
        }
        Ok(built)
    }

    /// Whether owner lists reached every stored child exactly once.
    fn complete(&self) -> bool {
        self.listed == self.items.len()
    }
}

fn insert_new<K: Eq + Hash, S>(
    items: &mut HashMap<K, S>,
    key: K,
    state: S,
) -> Result<(), ModelError> {
    if items.contains_key(&key) {
        return Err(ModelError::InvalidChange);
    }
    items.insert(key, state);
    Ok(())
}

fn remove_existing<K: Eq + Hash, S>(items: &mut HashMap<K, S>, key: K) -> Result<(), ModelError> {
    items
        .remove(&key)
        .map(|_| ())
        .ok_or(ModelError::InvalidChange)
}

fn id_key(key: ObjectKey) -> Result<Id, ModelError> {
    match key {
        ObjectKey::Id(id) => Ok(id),
        ObjectKey::Project | ObjectKey::Tempo(_) => Err(ModelError::InvalidChange),
    }
}

fn tempo_key(key: ObjectKey) -> Result<i64, ModelError> {
    match key {
        ObjectKey::Tempo(at) => Ok(at.get()),
        ObjectKey::Project | ObjectKey::Id(_) => Err(ModelError::InvalidChange),
    }
}

pub(super) struct Draft {
    project: ProjectState,
    tempos: HashMap<i64, TempoState>,
    tracks: HashMap<Id, TrackState>,
    contents: HashMap<Id, ContentState>,
    clips: Children<ClipState>,
    notes: Children<NoteState>,
    devices: Children<DeviceState>,
    parameters: Children<ParamState>,
    sends: Children<SendState>,
    sidechains: Children<SidechainState>,
}

impl Draft {
    pub(super) fn from_project(project: &Project) -> Self {
        let mut draft = Self {
            project: ProjectState::from_project(project),
            tempos: HashMap::new(),
            tracks: HashMap::new(),
            contents: HashMap::new(),
            clips: Children::new(),
            notes: Children::new(),
            devices: Children::new(),
            parameters: Children::new(),
            sends: Children::new(),
            sidechains: Children::new(),
        };
        for content in project.contents() {
            draft
                .contents
                .insert(content.id, ContentState::from_content(content));
            for note in &content.notes {
                draft
                    .notes
                    .items
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
                    .items
                    .insert(clip.id, ClipState::from_clip(track.id, clip));
            }
            draft.tracks.insert(track.id, TrackState::from_track(track));
            for send in &track.sends {
                draft
                    .sends
                    .items
                    .insert(send.id, SendState::from_send(track.id, send));
            }
            for device in &track.devices {
                draft
                    .devices
                    .items
                    .insert(device.id, DeviceState::from_device(track.id, device));
                for sidechain in &device.sidechains {
                    draft.sidechains.items.insert(
                        sidechain.id,
                        SidechainState::from_sidechain(device.id, sidechain),
                    );
                }
                for parameter in &device.parameters {
                    draft.parameters.items.insert(
                        parameter.id,
                        ParamState::from_parameter(device.id, parameter),
                    );
                }
            }
        }
        draft
    }

    pub(super) fn apply(&mut self, change: &Change) -> Result<(), ModelError> {
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

    // No catch-all arm here, in `remove`, or in `complete`: a new kind must pick
    // its store and say whether `finish` takes it whole or resolves it from owners.
    fn insert(
        &mut self,
        kind: ObjectKind,
        key: ObjectKey,
        fields: &[Field],
    ) -> Result<(), ModelError> {
        match kind {
            ObjectKind::TempoPoint => insert_new(
                &mut self.tempos,
                tempo_key(key)?,
                TempoState::from_fields(fields)?,
            ),
            ObjectKind::Track => insert_new(
                &mut self.tracks,
                id_key(key)?,
                TrackState::from_fields(fields)?,
            ),
            ObjectKind::Content => insert_new(
                &mut self.contents,
                id_key(key)?,
                ContentState::from_fields(fields)?,
            ),
            ObjectKind::Clip => self
                .clips
                .insert(id_key(key)?, ClipState::from_fields(fields)?),
            ObjectKind::Note => self
                .notes
                .insert(id_key(key)?, NoteState::from_fields(fields)?),
            ObjectKind::Device => self
                .devices
                .insert(id_key(key)?, DeviceState::from_fields(fields)?),
            ObjectKind::Parameter => self
                .parameters
                .insert(id_key(key)?, ParamState::from_fields(fields)?),
            ObjectKind::Send => self
                .sends
                .insert(id_key(key)?, SendState::from_fields(fields)?),
            ObjectKind::Sidechain => self
                .sidechains
                .insert(id_key(key)?, SidechainState::from_fields(fields)?),
        }
    }

    fn remove(&mut self, kind: ObjectKind, key: ObjectKey) -> Result<(), ModelError> {
        match kind {
            ObjectKind::TempoPoint => remove_existing(&mut self.tempos, tempo_key(key)?),
            ObjectKind::Track => remove_existing(&mut self.tracks, id_key(key)?),
            ObjectKind::Content => remove_existing(&mut self.contents, id_key(key)?),
            ObjectKind::Clip => self.clips.remove(id_key(key)?),
            ObjectKind::Note => self.notes.remove(id_key(key)?),
            ObjectKind::Device => self.devices.remove(id_key(key)?),
            ObjectKind::Parameter => self.parameters.remove(id_key(key)?),
            ObjectKind::Send => self.sends.remove(id_key(key)?),
            ObjectKind::Sidechain => self.sidechains.remove(id_key(key)?),
        }
    }

    /// Whether `finish` reached every stored object of `kind`.
    fn complete(&self, kind: ObjectKind) -> bool {
        match kind {
            // Finish takes every stored top-level object.
            ObjectKind::TempoPoint | ObjectKind::Track | ObjectKind::Content => true,
            ObjectKind::Clip => self.clips.complete(),
            ObjectKind::Note => self.notes.complete(),
            ObjectKind::Device => self.devices.complete(),
            ObjectKind::Parameter => self.parameters.complete(),
            ObjectKind::Send => self.sends.complete(),
            ObjectKind::Sidechain => self.sidechains.complete(),
        }
    }

    fn set(&mut self, id: ObjectKey, property: Property, value: &Value) -> Result<(), ModelError> {
        if let ObjectKey::Id(id) = id
            && let Some(parameter) = self.parameters.get_mut(&id)
        {
            return parameter.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(send) = self.sends.get_mut(&id)
        {
            return send.set(property, value);
        }
        if let ObjectKey::Id(id) = id
            && let Some(sidechain) = self.sidechains.get_mut(&id)
        {
            return sidechain.set(property, value);
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
            (ObjectKey::Id(id), Property::SidechainOrder, Value::Ids(ids)) => {
                self.devices
                    .get_mut(&id)
                    .ok_or(ModelError::InvalidChange)?
                    .sidechains = ids.clone();
                Ok(())
            }
            (ObjectKey::Id(_id), Property::Lane, Value::Text(_)) => Ok(()),
            _ => Err(ModelError::InvalidChange),
        }
    }

    pub(super) fn finish(mut self) -> Result<Project, ModelError> {
        let mut points: Vec<TempoPoint> = self
            .tempos
            .iter()
            .map(|(at, point)| point.finish(Tick::new(*at)))
            .collect();
        points.sort_by_key(|point| point.at);
        let tempo = TempoMap::try_from_points(points).ok_or(ModelError::InvalidChange)?;
        let mut seen = HashSet::<Id>::new();
        let mut contents = Vec::with_capacity(self.contents.len());
        let mut content_ids: Vec<Id> = self.contents.keys().copied().collect();
        content_ids.sort();
        for content_id in content_ids {
            if !seen.insert(content_id) {
                return Err(ModelError::DuplicateId);
            }
            let content = &self.contents[&content_id];
            let notes = self.notes.resolve(
                Ownership::CONTENT_NOTES,
                content_id,
                &content.note_order,
                &mut seen,
                |id, note, _| Ok(note.finish(id)),
            )?;
            contents.push(Arc::new(content.finish(content_id, notes)));
        }
        let mut tracks = Vec::with_capacity(self.tracks.len());
        for (track_id, track) in &self.tracks {
            if !seen.insert(*track_id) {
                return Err(ModelError::DuplicateId);
            }
            let clips = self.clips.resolve(
                Ownership::TRACK_CLIPS,
                *track_id,
                &track.clip_order,
                &mut seen,
                |id, clip, _| {
                    if !self.contents.contains_key(&clip.content) {
                        return Err(ModelError::InvalidChange);
                    }
                    Ok(clip.finish(id))
                },
            )?;
            let sends = self.sends.resolve(
                Ownership::TRACK_SENDS,
                *track_id,
                &track.send_order,
                &mut seen,
                |id, send, _| {
                    if !self.tracks.contains_key(&send.target) {
                        return Err(ModelError::InvalidChange);
                    }
                    send.finish(id)
                },
            )?;
            let sidechains = &mut self.sidechains;
            let parameters = &mut self.parameters;
            let devices = self.devices.resolve(
                Ownership::TRACK_DEVICES,
                *track_id,
                &track.device_order,
                &mut seen,
                |id, device, seen| {
                    #[cfg(test)]
                    crate::model_scale_tests::apply_device_owner_visit();
                    let sidechains = sidechains.resolve(
                        Ownership::DEVICE_SIDECHAINS,
                        id,
                        &device.sidechains,
                        seen,
                        |id, sidechain, _| {
                            if !self.tracks.contains_key(&sidechain.source) {
                                return Err(ModelError::InvalidChange);
                            }
                            sidechain.finish(id)
                        },
                    )?;
                    let parameters = parameters.resolve(
                        Ownership::DEVICE_PARAMETERS,
                        id,
                        &device.parameters,
                        seen,
                        |id, parameter, _| parameter.finish(id),
                    )?;
                    device.finish(id, sidechains, parameters)
                },
            )?;
            tracks.push(Arc::new(track.finish(*track_id, clips, devices, sends)));
        }
        if !ObjectKind::ALL.iter().all(|kind| self.complete(*kind)) {
            return Err(ModelError::InvalidChange);
        }
        let tracks = crate::model::arrange_tracks(&tracks)?;
        Ok(self.project.finish(tempo, contents, tracks))
    }
}
