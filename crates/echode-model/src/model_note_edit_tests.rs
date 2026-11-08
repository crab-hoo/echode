//! Copy budgets and fixture sizes constrain these tests, not product projects.

use super::note_duplicate::linked_fixture;
use super::{assert_runs_copied, measured};
use crate::{ClipKind, Content, Id, ModelError, NoteGeometry, Project, Tick};
use std::sync::Arc;

const MISSING: Id = Id::from_raw(1_000_001);

fn geometry(clip: &crate::Placement, note: &crate::Note) -> NoteGeometry {
    NoteGeometry {
        pitch: note.pitch,
        start: clip.sounding_start(note).unwrap(),
        length: note.length,
    }
}

fn assert_roundtrip(before: &Project, after: &Project) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let changes = before.diff(after);
    assert_eq!(before.apply_changeset(&changes), Ok(after.clone()));
    assert_eq!(after.apply_changeset(&changes.invert()), Ok(before.clone()));
}

#[test]
fn unchanged_geometry_does_not_copy_shared_content() {
    for count in [1, 64, 256] {
        for kind in [ClipKind::Fixed, ClipKind::Auto] {
            let project = linked_fixture(count, kind);
            let original = project.clone();
            let content = &project.contents()[0];
            for track in project.tracks() {
                let clip = &track.clips[0];
                for note in [&content.notes[0], content.notes.last().unwrap()] {
                    let (after, work) = measured(|| {
                        project
                            .edit_note(track.id, clip.id, note.id, geometry(clip, note))
                            .unwrap()
                    });
                    assert_eq!(
                        work.content_copies, 0,
                        "unchanged geometry cloned {} contents for {count} notes ({kind:?})",
                        work.content_copies
                    );
                    assert_eq!(work.content_run_copies, 0);
                    assert!(work.note_lookups <= count);
                    for (before, retained) in project.contents().iter().zip(after.contents()) {
                        assert!(Arc::ptr_eq(before, retained));
                    }
                    assert_eq!(after, project);
                    assert_roundtrip(&project, &after);
                }
            }
            assert_eq!(project, original);
        }
    }
}

#[test]
fn missing_geometry_target_is_rejected_before_copying_content() {
    for count in [0, 1, 64, 256] {
        let project = linked_fixture(count, ClipKind::Fixed);
        let original = project.clone();
        for track in project.tracks() {
            for missing in [MISSING, project.contents()[1].notes[0].id] {
                let (result, work) = measured(|| {
                    project.edit_note(
                        track.id,
                        track.clips[0].id,
                        missing,
                        NoteGeometry {
                            pitch: 60,
                            start: track.clips[0].start,
                            length: Tick::new(1),
                        },
                    )
                });
                assert_eq!(result, Err(ModelError::MissingNote));
                assert_eq!(
                    work.content_copies, 0,
                    "missing geometry target cloned {} contents for {count} notes",
                    work.content_copies
                );
                assert_eq!(work.content_run_copies, 0);
                assert_eq!(work.note_lookups, count);
            }
        }
        assert_eq!(project, original);
        project.check_invariants().unwrap();
    }
}

#[test]
fn changed_geometry_copies_content_once_and_preserves_other_fields() {
    for count in [1, 64, 256] {
        for kind in [ClipKind::Fixed, ClipKind::Auto] {
            let project = linked_fixture(count, kind);
            let original = project.clone();
            let content = &project.contents()[0];
            let note = content.notes.last().unwrap();
            for track in project.tracks() {
                let clip = &track.clips[0];
                let current = geometry(clip, note);
                for updated in [
                    NoteGeometry {
                        pitch: 67,
                        ..current
                    },
                    NoteGeometry {
                        start: Tick::new(current.start.get() - 1),
                        ..current
                    },
                    NoteGeometry {
                        length: Tick::new(2),
                        ..current
                    },
                ] {
                    let (after, work) = measured(|| {
                        project
                            .edit_note(track.id, clip.id, note.id, updated)
                            .unwrap()
                    });
                    assert_eq!(work.content_copies, 1);
                    assert_runs_copied(&work, content);
                    assert_eq!(work.note_lookups, count);
                    let mut expected = Content::clone(content);
                    expected.notes.edit(|notes| {
                        let target = notes.last_mut().unwrap();
                        target.pitch = updated.pitch;
                        target.start =
                            Tick::new(note.start.get() + updated.start.get() - current.start.get());
                        target.length = updated.length;
                    });
                    expected.notes.edit(|notes| crate::model::sort_notes(notes));
                    assert_eq!(after.content(content.id), Some(&expected));
                    assert!(Arc::ptr_eq(&project.contents()[1], &after.contents()[1]));
                    assert_roundtrip(&project, &after);
                }
            }
            assert_eq!(project, original);
        }
    }
}

#[test]
fn unchanged_geometry_still_refits_and_merges_adjacent_auto_windows() {
    let template = linked_fixture(1, ClipKind::Auto);
    let bar = template.meter().bar_ticks().unwrap().get();
    let mut track = (*template.tracks()[0]).clone();
    track.clips[1].kind = ClipKind::Auto;
    track.clips[1].start = Tick::new(bar);
    track.clips[1].length = Tick::new(bar);
    let project = Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        template
            .contents()
            .iter()
            .map(|item| Content::clone(item))
            .collect(),
        vec![track, (*template.tracks()[1]).clone()],
    )
    .unwrap();
    let original = project.clone();
    let note = &project.contents()[0].notes[0];
    let other = project.contents()[1].notes[0].id;
    for track in project.tracks() {
        let clip = &track.clips[0];
        let after = project
            .edit_note(track.id, clip.id, note.id, geometry(clip, note))
            .unwrap();
        assert_eq!(after.tracks()[0].clips.len(), 1);
        assert_eq!(after.tracks()[0].clips[0].length, Tick::new(2 * bar));
        let merged = after.content(after.tracks()[0].clips[0].content).unwrap();
        assert_eq!(merged.notes.len(), 2);
        assert_eq!(
            merged
                .notes
                .iter()
                .find(|item| item.id == other)
                .unwrap()
                .start,
            Tick::new(bar)
        );
        assert_eq!(after.tracks()[1].clips[0], project.tracks()[1].clips[0]);
        assert_roundtrip(&project, &after);
    }
    assert_eq!(project, original);
}

#[test]
fn geometry_rejection_keeps_validation_order_and_does_not_copy_content() {
    let project = linked_fixture(64, ClipKind::Fixed);
    let original = project.clone();
    let track = &project.tracks()[1];
    let clip = &track.clips[0];
    let note = &project.contents()[0].notes[0];
    let current = geometry(clip, note);
    for (owner, placement, target, updated, error) in [
        (MISSING, clip.id, note.id, current, ModelError::MissingTrack),
        (track.id, MISSING, note.id, current, ModelError::MissingClip),
        (
            project.tracks()[0].id,
            clip.id,
            note.id,
            current,
            ModelError::MissingClip,
        ),
        (
            MISSING,
            MISSING,
            MISSING,
            NoteGeometry {
                pitch: 128,
                ..current
            },
            ModelError::InvalidNote,
        ),
        (
            track.id,
            clip.id,
            MISSING,
            NoteGeometry {
                start: Tick::new(i64::MIN),
                ..current
            },
            ModelError::InvalidNote,
        ),
        (
            track.id,
            clip.id,
            note.id,
            NoteGeometry {
                length: Tick::new(0),
                ..current
            },
            ModelError::InvalidNote,
        ),
        (
            track.id,
            clip.id,
            note.id,
            NoteGeometry {
                start: Tick::new(i64::MAX),
                ..current
            },
            ModelError::InvalidNote,
        ),
    ] {
        let (result, work) = measured(|| project.edit_note(owner, placement, target, updated));
        assert_eq!(result, Err(error));
        assert_eq!(work.content_copies, 0);
        assert_eq!(work.content_run_copies, 0);
        assert_eq!(project, original);
    }
    project.check_invariants().unwrap();
}
