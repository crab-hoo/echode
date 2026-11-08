//! Lookup budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured};
use crate::{Content, Id, ModelError, Project, Tick, Track, midi7_to_velocity};
use std::sync::Arc;

const COPY: Id = Id::from_raw(1_000_000);
const MISSING: Id = Id::from_raw(1_000_001);

#[derive(Clone, Copy, Debug)]
enum Operation {
    Duplicate,
    Velocity,
}

impl Operation {
    fn apply(
        self,
        project: &Project,
        track: Id,
        clip: Id,
        note: Id,
    ) -> Result<Project, ModelError> {
        match self {
            Self::Duplicate => {
                let start = project
                    .tracks()
                    .iter()
                    .find(|owner| owner.id == track)
                    .and_then(|owner| owner.clips.iter().find(|item| item.id == clip))
                    .map_or(Tick::new(1), |clip| {
                        Tick::new(clip.start.get() - clip.offset.get() + 1)
                    });
                project.duplicate_note(track, clip, note, COPY, 67, start)
            }
            Self::Velocity => project.set_note_velocity(track, clip, note, 91),
        }
    }
}

fn linked_fixture(count: usize) -> Project {
    let project = fixture(count);
    let contents = project
        .contents()
        .iter()
        .map(|content| {
            let mut content = Content::clone(content);
            content.notes.edit(|notes| notes[0].velocity = 23_123);
            content.notes.edit(|notes| notes[0].release = 54_321);
            content
        })
        .collect();
    let mut track = Track::clone(&project.tracks()[0]);
    let mut linked = track.clone();
    linked.id = Id::from_raw(2_000_000);
    linked.order = crate::model::sibling_order(1);
    linked.devices = vec![crate::test_mixer(2_000_000)];
    for (index, (clip, link)) in track.clips.iter_mut().zip(&mut linked.clips).enumerate() {
        clip.gain = 0.25;
        clip.fade_in = 1;
        clip.fade_out = 2;
        *link = clip.clone();
        link.id = Id::from_raw(3_000_000 + index as u128);
        link.start = Tick::new(clip.start.get() + 10_000);
        link.offset = Tick::new(-7);
    }
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        vec![track, linked],
    )
    .unwrap()
}

fn assert_roundtrip(before: &Project, after: &Project) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let changes = before.diff(after);
    assert_eq!(before.apply_changeset(&changes), Ok(after.clone()));
    assert_eq!(after.apply_changeset(&changes.invert()), Ok(before.clone()));
}

fn check_lookup(operation: Operation) {
    for count in [1_usize, 2, 64, 256] {
        let project = linked_fixture(count);
        let original = project.clone();
        let track = &project.tracks()[0];
        // Probe both ends of the sorted content array through either shared owner.
        for index in [0, count - 1] {
            for owner in project.tracks() {
                let clip = &owner.clips[index];
                let content = project.content(clip.content).unwrap();
                let note = &content.notes[0];
                let (after, work) = measured(|| {
                    operation
                        .apply(&project, owner.id, clip.id, note.id)
                        .unwrap()
                });
                let searches = match operation {
                    Operation::Duplicate => 4,
                    Operation::Velocity => 1,
                };
                let budget = searches * (count.ilog2() as usize + 2);
                assert!(
                    work.content_keys > 0 && work.content_keys <= budget,
                    "{operation:?} inspected {} content keys for {count} contents (budget {budget})",
                    work.content_keys,
                );
                let changed = after.content(content.id).unwrap();
                let mut expected = content.clone();
                match operation {
                    Operation::Duplicate => {
                        let mut copy = note.clone();
                        copy.id = COPY;
                        copy.pitch = 67;
                        copy.start = Tick::new(1);
                        expected.notes.edit(|notes| notes.push(copy));
                    }
                    Operation::Velocity => {
                        expected
                            .notes
                            .edit(|notes| notes[0].velocity = midi7_to_velocity(91));
                    }
                }
                assert_eq!(changed, &expected);
                for old in project.contents() {
                    if old.id != content.id {
                        assert!(Arc::ptr_eq(old, after.shared_content(old.id).unwrap()));
                    }
                }
                // Fixed windows, offsets, fades and gain must not change in either owner.
                for (old, new) in project.tracks().iter().zip(after.tracks()) {
                    assert_eq!(old.clips, new.clips);
                }
                assert_eq!(project, original);
                assert_eq!(track.clips[index].content, owner.clips[index].content);
                assert_roundtrip(&project, &after);
            }
        }
    }
}

#[test]
fn duplicate_note_content_lookup_is_logarithmic() {
    check_lookup(Operation::Duplicate);
}

#[test]
fn velocity_content_lookup_is_logarithmic() {
    check_lookup(Operation::Velocity);
}

#[test]
fn note_content_lookup_preserves_target_errors_and_original_snapshot() {
    let project = linked_fixture(3);
    let original = project.clone();
    let track = &project.tracks()[0];
    let clip = &track.clips[1];
    let note = project.content(clip.content).unwrap().notes[0].id;
    let wrong_note = project.contents()[0].notes[0].id;
    for operation in [Operation::Duplicate, Operation::Velocity] {
        for (owner, placement, target, expected) in [
            (MISSING, clip.id, note, ModelError::MissingTrack),
            (track.id, MISSING, note, ModelError::MissingClip),
            (track.id, clip.id, MISSING, ModelError::MissingNote),
            (track.id, clip.id, wrong_note, ModelError::MissingNote),
            (
                track.id,
                project.tracks()[1].clips[1].id,
                note,
                ModelError::MissingClip,
            ),
        ] {
            assert_eq!(
                operation.apply(&project, owner, placement, target),
                Err(expected)
            );
        }
        // Invalid snapshots only exercise empty and interior missing-content lookups.
        for empty in [true, false] {
            let contents = project
                .contents()
                .iter()
                .filter(|content| !empty && content.id != clip.content)
                .map(|content| Content::clone(content))
                .collect();
            let malformed = Project::unchecked(
                project.meter(),
                project.tempo().clone(),
                contents,
                project
                    .tracks()
                    .iter()
                    .map(|track| Track::clone(track))
                    .collect(),
            );
            assert_eq!(
                operation.apply(&malformed, track.id, clip.id, note),
                Err(ModelError::MissingClip)
            );
        }
    }
    assert_eq!(
        project.set_note_velocity(MISSING, MISSING, MISSING, 128),
        Err(ModelError::InvalidNote)
    );
    assert_eq!(project, original);
    project.check_invariants().unwrap();
}
