//! Copy budgets and fixture sizes constrain these tests, not product projects.

use super::{measured, shared_fixture};
use crate::{Content, Id, ModelError, Project, Track, midi7_to_velocity, velocity_to_midi7};
use std::sync::Arc;

const MISSING: Id = Id::from_raw(1_000_000);

fn linked_fixture(notes: usize, velocity: u16) -> Project {
    let (template, mut contents, mut tracks) = shared_fixture(notes, 4);
    for note in &mut contents[0].notes {
        note.velocity = velocity;
        note.release = 54_321;
    }
    let (template, _) = template.add_track("linked");
    let mut linked = Track::clone(&template.tracks()[1]);
    linked.clips = tracks[0].clips.split_off(2);
    for clip in &mut linked.clips {
        clip.start = crate::Tick::new(clip.start.get() + 10_000);
        clip.offset = crate::Tick::new(-7);
        clip.gain = 0.25;
        clip.fade_in = 1;
        clip.fade_out = 2;
    }
    tracks.push(linked);
    Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks).unwrap()
}

fn assert_roundtrip(before: &Project, after: &Project) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let changes = before.diff(after);
    assert_eq!(before.apply_changeset(&changes), Ok(after.clone()));
    assert_eq!(after.apply_changeset(&changes.invert()), Ok(before.clone()));
}

#[test]
fn same_velocity_keeps_shared_content_without_copying_notes() {
    for count in [1, 64, 256] {
        for velocity in [0, 42, 127] {
            let project = linked_fixture(count, midi7_to_velocity(velocity));
            let content = &project.contents()[0];
            for track in project.tracks() {
                for note in [&content.notes[0], content.notes.last().unwrap()] {
                    let (after, work) = measured(|| {
                        project
                            .set_note_velocity(track.id, track.clips[0].id, note.id, velocity)
                            .unwrap()
                    });
                    assert_eq!(
                        work.velocity_note_copies, 0,
                        "same velocity copied {} notes for {count} notes",
                        work.velocity_note_copies
                    );
                    assert_eq!(work.velocity_content_copies, 0);
                    assert!(Arc::ptr_eq(content, &after.contents()[0]));
                    for (before, after) in project.tracks().iter().zip(after.tracks()) {
                        assert!(Arc::ptr_eq(before, after));
                    }
                    assert_eq!(project, after);
                    assert_roundtrip(&project, &after);
                }
            }
        }
    }
}

#[test]
fn missing_velocity_target_is_rejected_before_copying_notes() {
    for count in [0, 1, 64, 256] {
        let project = linked_fixture(count, midi7_to_velocity(42));
        let original = project.clone();
        for track in project.tracks() {
            let (result, work) =
                measured(|| project.set_note_velocity(track.id, track.clips[0].id, MISSING, 42));
            assert_eq!(result, Err(ModelError::MissingNote));
            assert_eq!(
                work.velocity_content_copies, 0,
                "missing velocity target copied {} contents for {count} notes",
                work.velocity_content_copies
            );
            assert_eq!(work.velocity_note_copies, 0);
        }
        assert_eq!(project, original);
        project.check_invariants().unwrap();
    }
}

#[test]
fn changed_velocity_copies_once_and_compares_full_resolution_values() {
    for count in [1, 64, 256] {
        // The MIDI7 projection matches 45, but setting 45 must still replace the
        // distinct full-resolution value rather than treating it as unchanged.
        let original_velocity = 23_123;
        let velocity = velocity_to_midi7(original_velocity);
        assert_ne!(midi7_to_velocity(velocity), original_velocity);
        let project = linked_fixture(count, original_velocity);
        let original = project.clone();
        let content = &project.contents()[0];
        let note = content.notes.last().unwrap();
        for track in project.tracks() {
            for velocity in [0, velocity, 127] {
                let (after, work) = measured(|| {
                    project
                        .set_note_velocity(track.id, track.clips[0].id, note.id, velocity)
                        .unwrap()
                });
                assert_eq!(work.velocity_content_copies, 1);
                assert_eq!(work.velocity_note_copies, count);
                assert!(!Arc::ptr_eq(content, &after.contents()[0]));
                let mut expected = Content::clone(content);
                expected.notes.last_mut().unwrap().velocity = midi7_to_velocity(velocity);
                assert_eq!(after.content(content.id), Some(&expected));
                for (before, after) in project.tracks().iter().zip(after.tracks()) {
                    assert!(Arc::ptr_eq(before, after));
                }
                assert_roundtrip(&project, &after);
            }
        }
        assert_eq!(project, original);
    }
}
