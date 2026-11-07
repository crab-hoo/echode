//! Insertion work budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured};
use crate::{ClipKind, Id, ModelError, NoteDraft, Project, Tick, Track};
use std::sync::Arc;

const NOTE: Id = Id::from_raw(1_000_000);
const CLIP: Id = Id::from_raw(1_000_001);
const CONTENT: Id = Id::from_raw(1_000_002);
const TAKE: Id = Id::from_raw(1_000_003);

fn spaced_fixture(count: usize, kind: ClipKind) -> Project {
    let project = fixture(count);
    let bar = project.meter().bar_ticks().unwrap().get();
    let mut contents: Vec<_> = project
        .contents()
        .iter()
        .map(|item| (**item).clone())
        .collect();
    contents[0].takes.push(TAKE);
    let mut track = Track::clone(&project.tracks()[0]);
    for (index, clip) in track.clips.iter_mut().enumerate() {
        clip.start = Tick::new(index as i64 * 4 * bar);
        clip.length = Tick::new(bar);
        clip.kind = kind;
    }
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        vec![track],
    )
    .unwrap()
}

fn draft(start: i64, length: i64) -> NoteDraft {
    NoteDraft {
        pitch: 67,
        velocity: 91,
        start: Tick::new(start),
        length: Tick::new(length),
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
fn insertion_reusing_or_merging_clips_checks_only_the_note_id() {
    for count in [64, 256] {
        for operation in ["fixed", "auto", "grow", "merge"] {
            let kind = if operation == "fixed" {
                ClipKind::Fixed
            } else {
                ClipKind::Auto
            };
            let project = spaced_fixture(count, kind);
            let original = project.clone();
            let track = &project.tracks()[0];
            let bar = project.meter().bar_ticks().unwrap().get();
            let note = match operation {
                "fixed" | "auto" => draft(1, 10),
                "grow" => draft(bar, 10),
                "merge" => draft(bar, 3 * bar + 1),
                _ => unreachable!(),
            };
            let (inserted, work) = measured(|| {
                project
                    .insert_note_with_id(track.id, NOTE, CLIP, CONTENT, note)
                    .unwrap()
            });
            assert_eq!(
                work.id_check_notes, count,
                "{operation} insertion scanned {} note IDs with {count} stored notes",
                work.id_check_notes,
            );
            assert_eq!(work.id_checks, 1);
            let owner = track.clips[0].content;
            let stored = inserted.content(owner).unwrap();
            assert_eq!(
                stored
                    .notes
                    .iter()
                    .find(|item| item.id == NOTE)
                    .unwrap()
                    .pitch,
                67
            );
            assert_eq!(stored.notes.len(), if operation == "merge" { 3 } else { 2 });
            assert_eq!(inserted.tracks()[0].clips[0].id, track.clips[0].id);
            assert!(inserted.content(CONTENT).is_none());
            assert!(
                !inserted.tracks()[0]
                    .clips
                    .iter()
                    .any(|item| item.id == CLIP)
            );
            let unaffected = if operation == "merge" { 2 } else { 1 };
            for content in &project.contents()[unaffected..] {
                assert!(Arc::ptr_eq(
                    content,
                    inserted.shared_content(content.id).unwrap()
                ));
            }
            assert_eq!(project, original);
            assert_roundtrip(&project, &inserted);
        }
    }
}

fn stored_ids(project: &Project) -> Vec<Id> {
    let content = &project.contents()[0];
    let track = &project.tracks()[0];
    vec![
        content.id,
        content.notes[0].id,
        TAKE,
        track.id,
        track.clips[0].id,
        track.devices[0].id,
        track.devices[0].parameters[0].id,
    ]
}

#[test]
fn unused_clip_and_content_ids_do_not_reject_reused_or_merged_clips() {
    for operation in ["fixed", "auto", "grow", "merge"] {
        let kind = if operation == "fixed" {
            ClipKind::Fixed
        } else {
            ClipKind::Auto
        };
        let project = spaced_fixture(2, kind);
        let track = &project.tracks()[0];
        let bar = project.meter().bar_ticks().unwrap().get();
        let note = match operation {
            "fixed" | "auto" => draft(1, 10),
            "grow" => draft(bar, 10),
            "merge" => draft(bar, 3 * bar + 1),
            _ => unreachable!(),
        };
        // These IDs are ignored when no new placement/content is created,
        // including when they collide with the new note or with one another.
        for id in stored_ids(&project).into_iter().chain([NOTE, CLIP]) {
            for (clip, content) in [(id, CONTENT), (CLIP, id), (id, id)] {
                let (inserted, work) = measured(|| {
                    project
                        .insert_note_with_id(track.id, NOTE, clip, content, note)
                        .unwrap()
                });
                assert_eq!(work.id_checks, 1, "{operation} checked ignored IDs");
                assert_roundtrip(&project, &inserted);
            }
        }
    }
}

#[test]
fn new_auto_clips_check_and_preserve_all_three_ids() {
    for count in [64, 256] {
        let project = spaced_fixture(count, ClipKind::Fixed);
        let track = &project.tracks()[0];
        let bar = project.meter().bar_ticks().unwrap().get();
        let (inserted, work) = measured(|| {
            project
                .insert_note_with_id(track.id, NOTE, CLIP, CONTENT, draft(2 * bar, 10))
                .unwrap()
        });
        assert_eq!(work.id_checks, 3);
        assert_eq!(work.id_check_notes, 3 * count);
        let created = inserted.tracks()[0]
            .clips
            .iter()
            .find(|item| item.id == CLIP)
            .unwrap();
        assert_eq!(created.kind, ClipKind::Auto);
        assert_eq!(created.content, CONTENT);
        assert_eq!(created.start, Tick::new(2 * bar));
        assert_eq!(created.length, Tick::new(bar));
        assert_eq!(inserted.content(CONTENT).unwrap().notes[0].id, NOTE);
        for content in project.contents() {
            assert!(Arc::ptr_eq(
                content,
                inserted.shared_content(content.id).unwrap()
            ));
        }
        assert_roundtrip(&project, &inserted);
    }
    let (empty, track) = Project::new().add_track("empty");
    let (inserted, work) = measured(|| {
        empty
            .insert_note_with_id(track, NOTE, CLIP, CONTENT, draft(0, 10))
            .unwrap()
    });
    assert_eq!(work.id_checks, 3);
    assert_eq!(work.id_check_notes, 0);
    assert_eq!(inserted.tracks()[0].clips[0].id, CLIP);
    assert_roundtrip(&empty, &inserted);
}

#[test]
fn new_auto_clip_ids_cannot_collide_with_any_stored_identity_or_each_other() {
    let project = spaced_fixture(2, ClipKind::Fixed);
    let original = project.clone();
    let track = &project.tracks()[0];
    let bar = project.meter().bar_ticks().unwrap().get();
    for used in stored_ids(&project) {
        for (note, clip, content) in [
            (used, CLIP, CONTENT),
            (NOTE, used, CONTENT),
            (NOTE, CLIP, used),
        ] {
            assert_eq!(
                project.insert_note_with_id(track.id, note, clip, content, draft(2 * bar, 10)),
                Err(ModelError::DuplicateId),
            );
            assert_eq!(project, original);
        }
        // The note itself must always be unique, even inside an existing window.
        assert_eq!(
            project.insert_note_with_id(track.id, used, CLIP, CONTENT, draft(1, 10)),
            Err(ModelError::DuplicateId),
        );
    }
    for (note, clip, content) in [
        (NOTE, NOTE, CONTENT),
        (NOTE, CLIP, NOTE),
        (NOTE, CLIP, CLIP),
    ] {
        assert_eq!(
            project.insert_note_with_id(track.id, note, clip, content, draft(2 * bar, 10)),
            Err(ModelError::DuplicateId),
        );
    }
    assert_eq!(project, original);
    project.check_invariants().unwrap();
}

#[test]
fn invalid_insertion_keeps_error_precedence_without_checking_unused_ids() {
    let project = spaced_fixture(2, ClipKind::Fixed);
    let track = project.tracks()[0].id;
    let used = project.contents()[0].notes[0].id;
    let missing = Id::from_raw(0);
    for (track, note, input, expected, checks) in [
        (track, used, draft(0, 0), ModelError::InvalidNote, 0),
        (missing, used, draft(0, 10), ModelError::DuplicateId, 1),
        (missing, NOTE, draft(0, 10), ModelError::MissingTrack, 1),
    ] {
        let (result, work) =
            measured(|| project.insert_note_with_id(track, note, CLIP, CONTENT, input));
        assert_eq!(result, Err(expected));
        assert_eq!(work.id_checks, checks);
    }
}
