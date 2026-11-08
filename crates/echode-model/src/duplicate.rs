//! Copying a track subtree with independent clip contents.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::id::Id;
use crate::model::{Content, ModelError, Project, Track, arrange_tracks, sort_notes};

/// Domain separation for [`copy_id`]. A new derivation needs a new context.
const COPY_CONTEXT: &str = "echode 2026-10-20 track.duplicate object id";

/// Id of the copy of `original` made by the duplicate edit named `seed`.
fn copy_id(seed: Id, original: Id) -> Id {
    derived_id(COPY_CONTEXT, seed, original)
}

/// Id derived from an edit's `seed` and an `original` id under `context`.
///
/// BLAKE3 in key-derivation mode keeps the result a function of both ids, so
/// replaying the action rebuilds the same ids. The digest becomes a UUIDv8.
/// Each kind of derived object uses its own context, so two derivations from
/// one seed and one original never meet.
pub(crate) fn derived_id(context: &str, seed: Id, original: Id) -> Id {
    let mut hasher = blake3::Hasher::new_derive_key(context);
    hasher.update(&seed.raw().to_be_bytes());
    hasher.update(&original.raw().to_be_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Id::from_raw(
        uuid::Builder::from_custom_bytes(bytes)
            .into_uuid()
            .as_u128(),
    )
}

impl Project {
    /// Copies `source` and its descendants right after the source subtree,
    /// under the same parent. Only the copied root receives a new order key.
    ///
    /// Each copied track, device, parameter, placement, content, and note gets
    /// [`copy_id`] of its original. Placements in the copy point at new
    /// contents with new note ids, so an edit to the copy never reaches the
    /// original. Placements in the copy that shared a content share its copy.
    /// Names, plugin ids, saved plugin state, bypass, offline, mixer values,
    /// clip windows, gains, and fades are kept.
    pub fn duplicate_track(&self, source: Id, seed: Id) -> Result<Self, ModelError> {
        let first = self.track_index(source)?;
        let tracks = self.tracks();
        // The arrange order keeps a subtree contiguous after its root.
        let mut members = HashSet::from([source]);
        let mut end = first + 1;
        while let Some(track) = tracks.get(end)
            && track.parent.is_some_and(|parent| members.contains(&parent))
        {
            members.insert(track.id);
            end += 1;
        }
        let mut used = self.used_ids();
        let mut fresh = |original: Id| {
            let id = copy_id(seed, original);
            if used.insert(id) {
                Ok(id)
            } else {
                Err(ModelError::DuplicateId)
            }
        };
        let mut copied_contents: HashMap<Id, Id> = HashMap::new();
        let mut contents = self.contents().to_vec();
        let mut copies = Vec::with_capacity(end - first);
        for track in &tracks[first..end] {
            let mut copy = Track::clone(track);
            copy.id = fresh(track.id)?;
            if track.id == source {
                copy.order = self.order_after(track)?;
            } else {
                copy.parent = track.parent.map(|parent| copy_id(seed, parent));
            }
            for device in &mut copy.devices {
                device.id = fresh(device.id)?;
                for parameter in &mut device.parameters {
                    parameter.id = fresh(parameter.id)?;
                }
            }
            for clip in &mut copy.clips {
                clip.id = fresh(clip.id)?;
                clip.content = match copied_contents.get(&clip.content) {
                    Some(&content) => content,
                    None => {
                        let original = self.content(clip.content).ok_or(ModelError::MissingClip)?;
                        let mut content = Content::clone(original);
                        content.id = fresh(original.id)?;
                        content.notes.edit(|notes| {
                            for note in notes.iter_mut() {
                                note.id = fresh(note.id)?;
                            }
                            // Notes tie-break on id, so new ids can change their order.
                            sort_notes(notes);
                            Ok::<_, ModelError>(())
                        })?;
                        copied_contents.insert(original.id, content.id);
                        let id = content.id;
                        contents.push(Arc::new(content));
                        id
                    }
                };
            }
            copies.push(Arc::new(copy));
        }
        contents.sort_by_key(|content| content.id);
        let mut all = tracks.to_vec();
        all.extend(copies);
        Ok(self.with_parts(contents, arrange_tracks(&all)?))
    }

    /// Every id the snapshot holds, including content takes.
    pub(crate) fn used_ids(&self) -> HashSet<Id> {
        #[cfg(test)]
        crate::model_scale_tests::used_id_collection();
        let mut used = HashSet::new();
        for content in self.contents() {
            used.insert(content.id);
            used.extend(content.takes.iter().copied());
            used.extend(content.notes.iter().map(|note| {
                #[cfg(test)]
                crate::model_scale_tests::used_id_note();
                note.id
            }));
        }
        for track in self.tracks() {
            used.insert(track.id);
            used.extend(track.clips.iter().map(|clip| clip.id));
            for device in &track.devices {
                used.insert(device.id);
                used.extend(device.parameters.iter().map(|parameter| parameter.id));
            }
        }
        used
    }
}

#[cfg(test)]
mod tests {
    use super::copy_id;
    use crate::id::Id;

    #[test]
    fn copy_ids_depend_on_both_ids_and_are_version_8() {
        let (seed, original) = (Id::from_raw(1), Id::from_raw(2));
        assert_eq!(copy_id(seed, original), copy_id(seed, original));
        assert_ne!(copy_id(seed, original), copy_id(original, seed));
        assert_ne!(copy_id(seed, original), copy_id(Id::from_raw(3), original));
        let uuid = uuid::Uuid::from_u128(copy_id(seed, original).raw());
        assert_eq!(uuid.get_version_num(), 8);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
    }
}
