//! Track insertion budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured};
use crate::{Id, MixerIds, ModelError, Project};
use std::sync::Arc;

const TRACK: Id = Id::from_raw(1_000_000);
const MIXER: MixerIds = MixerIds::from_base(1_000_001);
const TAKE: Id = Id::from_raw(1_000_006);
const FX: Id = Id::from_raw(1_000_007);

#[derive(Clone, Copy, Debug)]
enum Insertion {
    Append,
    Child,
    Sibling,
    IntoFolder,
    AfterLeaf,
}

const INSERTIONS: [Insertion; 5] = [
    Insertion::Append,
    Insertion::Child,
    Insertion::Sibling,
    Insertion::IntoFolder,
    Insertion::AfterLeaf,
];

fn insert(
    project: &Project,
    insertion: Insertion,
    id: Id,
    mixer: MixerIds,
) -> Result<Project, ModelError> {
    let anchor = project.tracks()[0].id;
    match insertion {
        Insertion::Append => project.add_track_with_id(id, "new", mixer),
        Insertion::Child => project.add_child_with_id(id, anchor, "new", mixer),
        Insertion::Sibling => project.insert_track_after_with_id(id, anchor, false, "new", mixer),
        Insertion::IntoFolder => project.insert_track_after_with_id(id, anchor, true, "new", mixer),
        Insertion::AfterLeaf => {
            project.insert_track_after_with_id(id, project.tracks()[1].id, true, "new", mixer)
        }
    }
}

fn populated(count: usize) -> Project {
    let project = fixture(count);
    let mut contents: Vec<_> = project
        .contents()
        .iter()
        .map(|content| (**content).clone())
        .collect();
    contents[0].takes.push(TAKE);
    let project = Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        project
            .tracks()
            .iter()
            .map(|track| (**track).clone())
            .collect(),
    )
    .unwrap();
    let anchor = project.tracks()[0].id;
    let project = project
        .add_clap_device(anchor, FX, "org.example.fx")
        .unwrap();
    let project = project.add_child(anchor, "leaf").unwrap().0;
    project.add_track("unrelated").0
}

fn round_trip(before: &Project, after: &Project) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let changes = before.diff(after);
    assert_eq!(before.apply_changeset(&changes).unwrap(), *after);
    assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), *before);
}

#[test]
fn track_insertions_check_existing_ids_in_one_pass() {
    for count in [64, 256] {
        let project = populated(count);
        let original = project.clone();
        for insertion in INSERTIONS {
            let (after, work) = measured(|| insert(&project, insertion, TRACK, MIXER).unwrap());
            assert_eq!(
                work.id_check_notes, count,
                "{insertion:?} scanned {} note IDs for {count} stored notes; track IDs must be checked together",
                work.id_check_notes,
            );
            assert_eq!(work.id_checks, 1, "{insertion:?} repeated the ID scan");
            assert_eq!(
                work.used_id_collections, 0,
                "insertion collected all project IDs"
            );
            let created = after
                .tracks()
                .iter()
                .find(|track| track.id == TRACK)
                .unwrap();
            assert_eq!(created.name, "new");
            assert!(created.clips.is_empty());
            assert_eq!(created.devices, vec![crate::Device::unity(MIXER)]);
            let parent = match insertion {
                Insertion::Append | Insertion::Sibling => None,
                Insertion::Child | Insertion::IntoFolder | Insertion::AfterLeaf => {
                    Some(project.tracks()[0].id)
                }
            };
            assert_eq!(created.parent, parent);
            let expected_position = match insertion {
                Insertion::Append => after.tracks().len() - 1,
                _ => 2,
            };
            assert_eq!(after.tracks()[expected_position].id, TRACK);
            for track in project.tracks() {
                let kept = after
                    .tracks()
                    .iter()
                    .find(|item| item.id == track.id)
                    .unwrap();
                assert!(Arc::ptr_eq(track, kept));
            }
            for content in project.contents() {
                assert!(Arc::ptr_eq(
                    content,
                    after.shared_content(content.id).unwrap()
                ));
            }
            assert_eq!(project, original);
            round_trip(&project, &after);
        }
    }
}

fn ids() -> [Id; 6] {
    [
        TRACK,
        MIXER.device,
        MIXER.gain,
        MIXER.pan,
        MIXER.mute,
        MIXER.solo,
    ]
}

fn from_ids(ids: [Id; 6]) -> (Id, MixerIds) {
    let [track, device, gain, pan, mute, solo] = ids;
    (
        track,
        MixerIds {
            device,
            gain,
            pan,
            mute,
            solo,
        },
    )
}

#[test]
fn track_insertions_reject_collisions_with_every_existing_object_kind() {
    let project = populated(4);
    let original = project.clone();
    let track = &project.tracks()[0];
    let content = project.contents().last().unwrap();
    let mixer = track.devices.last().unwrap();
    let occupied = [
        content.id,
        content.notes[0].id,
        TAKE,
        track.id,
        track.clips.last().unwrap().id,
        FX,
        mixer.id,
        mixer.parameters[0].id,
        mixer.parameters[3].id,
        project.tracks()[1].id,
    ];
    for insertion in INSERTIONS {
        for slot in 0..ids().len() {
            for collision in occupied {
                let mut candidates = ids();
                candidates[slot] = collision;
                let (track, mixer) = from_ids(candidates);
                let (result, work) = measured(|| insert(&project, insertion, track, mixer));
                assert_eq!(
                    result,
                    Err(ModelError::DuplicateId),
                    "{insertion:?}, slot {slot}"
                );
                assert!(
                    work.id_checks <= 1,
                    "collision detection repeated the ID scan"
                );
                assert_eq!(project, original);
            }
        }
    }
}

#[test]
fn track_insertions_reject_every_pair_of_duplicate_candidate_ids() {
    let project = populated(4);
    let original = project.clone();
    for insertion in INSERTIONS {
        for left in 0..ids().len() {
            for right in left + 1..ids().len() {
                let mut candidates = ids();
                candidates[right] = candidates[left];
                let (track, mixer) = from_ids(candidates);
                let (result, work) = measured(|| insert(&project, insertion, track, mixer));
                assert_eq!(
                    result,
                    Err(ModelError::DuplicateId),
                    "{insertion:?}, {left}/{right}"
                );
                assert_eq!(
                    work.id_checks, 0,
                    "duplicate candidates scanned the project"
                );
                assert_eq!(project, original);
            }
        }
    }
}

#[test]
fn track_insertions_keep_unsorted_candidate_ids_and_integer_endpoints() {
    let project = populated(4);
    let candidates = [
        Id::from_raw(0),
        Id::from_raw(u128::MAX),
        Id::from_raw(42),
        Id::from_raw(u128::MAX - 1),
        Id::from_raw(17),
        Id::from_raw(1 << 100),
    ];
    for rotation in 0..candidates.len() {
        let mut rotated = candidates;
        rotated.rotate_left(rotation);
        let (id, mixer) = from_ids(rotated);
        for insertion in INSERTIONS {
            let after = insert(&project, insertion, id, mixer).unwrap();
            let created = after.tracks().iter().find(|track| track.id == id).unwrap();
            assert_eq!(created.devices, vec![crate::Device::unity(mixer)]);
            round_trip(&project, &after);
        }
    }
}

#[test]
fn empty_projects_and_missing_anchors_keep_their_insertion_errors() {
    let empty = Project::new();
    let (after, work) = measured(|| empty.add_track_with_id(TRACK, "first", MIXER).unwrap());
    assert_eq!(work.id_checks, 1);
    assert_eq!(work.id_check_notes, 0);
    round_trip(&empty, &after);
    let missing = Id::from_raw(2_000_000);
    assert_eq!(
        empty.add_child_with_id(TRACK, missing, "new", MIXER),
        Err(ModelError::MissingTrack)
    );
    // Child insertion checks candidate collisions before checking the parent;
    // positioned insertion resolves its anchor first, as before.
    let (duplicate, mixer) =
        from_ids([TRACK, TRACK, MIXER.gain, MIXER.pan, MIXER.mute, MIXER.solo]);
    assert_eq!(
        empty.add_child_with_id(duplicate, missing, "new", mixer),
        Err(ModelError::DuplicateId)
    );
    assert_eq!(
        empty.insert_track_after_with_id(duplicate, missing, true, "new", mixer),
        Err(ModelError::MissingTrack)
    );
}
