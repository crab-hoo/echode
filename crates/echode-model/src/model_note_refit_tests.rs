//! Actual note-copy budgets and fixture sizes constrain these tests, not product projects.

use super::measured;
use super::note_duplicate::linked_fixture;
use crate::{ClipKind, Content, Id, NoteDraft, NoteGeometry, Project, Tick, Track};
use std::sync::Arc;

const NOTE: Id = Id::from_raw(1_000_000);

fn shifted_fixture(count: usize, origin: i64) -> Project {
    let template = linked_fixture(count, ClipKind::Auto);
    let mut tracks: Vec<_> = template
        .tracks()
        .iter()
        .map(|item| Track::clone(item))
        .collect();
    tracks[0].clips[0].start = Tick::new(origin);
    Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        template
            .contents()
            .iter()
            .map(|item| Content::clone(item))
            .collect(),
        tracks,
    )
    .unwrap()
}

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

fn assert_auto_bounds(project: &Project, origin: i64) {
    let clip = &project.tracks()[0].clips[0];
    let content = project.content(clip.content).unwrap();
    let bar = i128::from(project.meter().bar_ticks().unwrap().get());
    let first = content
        .notes
        .iter()
        .map(|note| i128::from(note.start.get()).div_euclid(bar))
        .min()
        .unwrap();
    let last = content
        .notes
        .iter()
        .map(|note| {
            (i128::from(note.start.get()) + i128::from(note.length.get()) - 1).div_euclid(bar)
        })
        .max()
        .unwrap();
    assert_eq!(clip.kind, ClipKind::Auto);
    assert_eq!(
        i128::from(clip.start.get()),
        i128::from(origin) + first * bar
    );
    assert_eq!(i128::from(clip.offset.get()), first * bar);
    assert_eq!(i128::from(clip.length.get()), (last - first + 1) * bar);
}

#[test]
fn note_clone_counter_preserves_all_registered_fields() {
    let note = crate::Note {
        id: NOTE,
        start: Tick::new(i64::MIN),
        length: Tick::new(i64::MAX),
        pitch: u8::MAX,
        velocity: 23_123,
        release: 54_321,
        articulation: Some(Id::from_raw(1_000_001)),
    };
    let (copied, work) = measured(|| note.clone());
    assert_eq!(copied, note);
    assert_eq!(work.note_copies, 1);
}

#[test]
fn unchanged_auto_refit_borrows_notes_without_copying_them() {
    for count in [1, 64, 256] {
        for origin in [-123_457, 0, 234_567] {
            let project = shifted_fixture(count, origin);
            for track in project.tracks() {
                let clip = &track.clips[0];
                let note = &project.contents()[0].notes[0];
                let (after, work) = measured(|| {
                    project
                        .edit_note(track.id, clip.id, note.id, geometry(clip, note))
                        .unwrap()
                });
                assert_eq!(
                    work.note_copies, 0,
                    "unchanged auto refit copied {} notes for {count} stored notes at origin {origin}",
                    work.note_copies
                );
                assert_eq!(after, project);
                for (before, retained) in project.contents().iter().zip(after.contents()) {
                    assert!(Arc::ptr_eq(before, retained));
                }
                assert_auto_bounds(&after, origin);
                assert_roundtrip(&project, &after);
            }
        }
    }
}

#[test]
fn auto_refit_copies_only_notes_needed_for_the_edit() {
    for count in [2, 64, 256] {
        for origin in [-123_457, 0, 234_567] {
            let project = shifted_fixture(count, origin);
            let original = project.clone();
            let content = &project.contents()[0];
            let note = &content.notes[0];
            let bar = project.meter().bar_ticks().unwrap();
            for track in project.tracks() {
                let clip = &track.clips[0];
                let current = geometry(clip, note);
                for operation in ["edit", "insert", "duplicate", "delete"] {
                    let (after, work) = measured(|| {
                        match operation {
                            "edit" => project.edit_note(
                                track.id,
                                clip.id,
                                note.id,
                                NoteGeometry {
                                    pitch: 67,
                                    start: Tick::new(current.start.get() - 1),
                                    ..current
                                },
                            ),
                            "insert" => project.insert_note_with_id(
                                track.id,
                                NOTE,
                                Id::from_raw(1_000_001),
                                Id::from_raw(1_000_002),
                                NoteDraft {
                                    pitch: 67,
                                    velocity: 91,
                                    start: Tick::new(current.start.get() + 1),
                                    length: bar,
                                },
                            ),
                            "duplicate" => project.duplicate_note(
                                track.id,
                                clip.id,
                                note.id,
                                NOTE,
                                67,
                                Tick::new(current.start.get() - 1),
                            ),
                            "delete" => project.delete_note(track.id, note.id),
                            _ => unreachable!(),
                        }
                        .unwrap()
                    });
                    assert_eq!(
                        work.note_copies,
                        count + usize::from(operation == "duplicate"),
                        "{operation} auto refit copied {} notes for {count} stored notes at origin {origin}",
                        work.note_copies
                    );
                    let mut expected = Content::clone(content);
                    match operation {
                        "edit" => {
                            expected.notes[0].pitch = 67;
                            expected.notes[0].start = Tick::new(-1);
                        }
                        "insert" => expected.notes.push(crate::Note {
                            id: NOTE,
                            pitch: 67,
                            velocity: crate::midi7_to_velocity(91),
                            release: 0,
                            start: Tick::new(1),
                            length: bar,
                            articulation: None,
                        }),
                        "duplicate" => {
                            let mut copied = note.clone();
                            copied.id = NOTE;
                            copied.pitch = 67;
                            copied.start = Tick::new(-1);
                            expected.notes.push(copied);
                        }
                        "delete" => {
                            expected.notes.remove(0);
                        }
                        _ => unreachable!(),
                    }
                    crate::model::sort_notes(&mut expected.notes);
                    assert_eq!(after.content(content.id), Some(&expected));
                    assert!(Arc::ptr_eq(&project.contents()[1], &after.contents()[1]));
                    let link = &project.tracks()[1].clips[0];
                    if track.id != project.tracks()[1].id
                        || operation != "duplicate" && operation != "edit"
                    {
                        assert_eq!(&after.tracks()[1].clips[0], link);
                    }
                    assert_auto_bounds(&after, origin);
                    assert_roundtrip(&project, &after);
                    assert_eq!(project, original);
                }
            }
        }
    }
}

#[test]
fn merged_auto_refit_copies_each_original_note_once() {
    for count in [1, 64, 256] {
        let template = linked_fixture(count, ClipKind::Auto);
        let bar = template.meter().bar_ticks().unwrap();
        let mut tracks: Vec<_> = template
            .tracks()
            .iter()
            .map(|item| Track::clone(item))
            .collect();
        tracks[0].clips[1].kind = ClipKind::Auto;
        tracks[0].clips[1].start = bar;
        tracks[0].clips[1].length = bar;
        let project = Project::try_assemble(
            template.meter(),
            template.tempo().clone(),
            template
                .contents()
                .iter()
                .map(|item| Content::clone(item))
                .collect(),
            tracks,
        )
        .unwrap();
        let note = &project.contents()[0].notes[0];
        let other = &project.contents()[1].notes[0];
        for track in project.tracks() {
            let clip = &track.clips[0];
            let (after, work) = measured(|| {
                project
                    .edit_note(track.id, clip.id, note.id, geometry(clip, note))
                    .unwrap()
            });
            assert_eq!(
                work.note_copies,
                count + 1,
                "merged auto refit copied {} notes for {} original notes",
                work.note_copies,
                count + 1
            );
            assert_eq!(after.tracks()[0].clips.len(), 1);
            assert_eq!(after.tracks()[1].clips, project.tracks()[1].clips);
            let mut expected = Content::clone(&project.contents()[0]);
            let mut absorbed = other.clone();
            absorbed.start = bar;
            expected.notes.push(absorbed);
            assert_eq!(after.contents().len(), 1);
            assert_eq!(after.content(expected.id), Some(&expected));
            assert_auto_bounds(&after, 0);
            assert_roundtrip(&project, &after);
        }
    }
}
