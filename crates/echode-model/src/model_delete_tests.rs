//! Deletion work budgets and fixture sizes constrain these tests, not product projects.

use super::{fixture, measured, shared_fixture};
use crate::{ClipKind, Content, Id, ModelError, Project, Tick, Track};
use std::sync::Arc;

fn track(id: u128, order: u32) -> Track {
    Track {
        id: Id::from_raw(id),
        name: format!("delete {id}"),
        parent: None,
        order: crate::model::sibling_order(order),
        devices: vec![crate::test_mixer(id)],
        clips: Vec::new(),
    }
}

fn search_fixture(notes: usize, placements: usize) -> Project {
    let (template, mut contents, mut tracks) = shared_fixture(notes, placements);
    let mut second = contents[0].clone();
    second.id = Id::from_raw(4);
    for (index, note) in second.notes.iter_mut().enumerate() {
        note.id = Id::from_raw(30_000 + index as u128);
    }
    let mut last = contents[0].clone();
    last.id = Id::from_raw(7);
    last.notes.truncate(1);
    last.notes[0].id = Id::from_raw(40_000);
    last.notes[0].velocity = 23_123;
    last.notes[0].release = 54_321;
    contents.extend([second, last]);
    for (index, clip) in tracks[0].clips.iter_mut().enumerate() {
        if index % 2 == 1 {
            clip.content = Id::from_raw(4);
        }
    }
    let mut last_clip = tracks[0].clips[0].clone();
    last_clip.id = Id::from_raw(50_000);
    last_clip.content = Id::from_raw(7);
    last_clip.start = Tick::new(placements as i64 * 100);
    last_clip.gain = 0.25;
    last_clip.fade_in = 1;
    last_clip.fade_out = 2;
    tracks[0].clips.push(last_clip);
    tracks[0].order = crate::model::sibling_order(0);
    let mut elsewhere = track(60_000, 1);
    let mut link = tracks[0].clips[0].clone();
    link.id = Id::from_raw(50_001);
    elsewhere.clips.push(link);
    tracks.push(elsewhere);
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
fn deletion_search_visits_each_shared_content_once() {
    for (notes, placements) in [(64, 256), (256, 64)] {
        let project = search_fixture(notes, placements);
        let track = &project.tracks()[0];
        for (content_id, note_id, expected) in [
            (1, 20_000 + notes as u128 - 1, notes),
            (4, 30_000 + notes as u128 - 1, 2 * notes),
            (7, 40_000, 2 * notes + 1),
        ] {
            let owner = Id::from_raw(content_id);
            let note = Id::from_raw(note_id);
            let (deleted, work) = measured(|| project.delete_note(track.id, note).unwrap());
            assert_eq!(
                work.note_lookups, expected,
                "deletion searched {} notes for {notes} notes per shared content and {placements} placements",
                work.note_lookups,
            );
            assert_eq!(deleted.tracks()[0].clips, track.clips);
            assert!(Arc::ptr_eq(&project.tracks()[1], &deleted.tracks()[1]));
            for original in project.contents() {
                let remaining = deleted.shared_content(original.id).unwrap();
                if original.id == owner {
                    let mut expected = Content::clone(original);
                    expected.notes.retain(|item| item.id != note);
                    assert_eq!(remaining.as_ref(), &expected);
                    assert_eq!(
                        original.notes.len(),
                        if content_id == 7 { 1 } else { notes }
                    );
                } else {
                    assert!(Arc::ptr_eq(original, remaining));
                }
            }
            assert_roundtrip(&project, &deleted);
        }
    }
}

#[test]
fn missing_note_search_is_bounded_and_stays_on_the_requested_track() {
    for (notes, placements) in [(64, 256), (256, 64)] {
        let project = search_fixture(notes, placements);
        let original = project.clone();
        for (track, note, expected) in [
            (project.tracks()[0].id, Id::from_raw(0), 2 * notes + 1),
            (project.tracks()[0].id, Id::from_raw(50_000), 2 * notes + 1),
            (project.tracks()[1].id, Id::from_raw(40_000), notes),
        ] {
            let (error, work) = measured(|| project.delete_note(track, note).unwrap_err());
            assert_eq!(error, ModelError::MissingNote);
            assert_eq!(
                work.note_lookups, expected,
                "missing-note deletion searched {} notes for {notes} notes per shared content and {placements} placements",
                work.note_lookups,
            );
            assert_eq!(work.delete_track_copies, 0);
            assert_eq!(project, original);
        }
        let (error, work) = measured(|| project.delete_note(Id::from_raw(0), Id::from_raw(40_000)));
        assert_eq!(error, Err(ModelError::MissingTrack));
        assert_eq!(work.note_lookups, 0);
    }
}

#[test]
fn missing_note_search_resolves_each_empty_content_once() {
    let project = search_fixture(1, 256);
    let track = project.tracks()[0].id;
    let project = [20_000, 30_000, 40_000]
        .into_iter()
        .fold(project, |project, note| {
            project.delete_note(track, Id::from_raw(note)).unwrap()
        });
    let (error, work) = measured(|| project.delete_note(track, Id::from_raw(0)));
    assert_eq!(error, Err(ModelError::MissingNote));
    assert_eq!(work.note_lookups, 0);
    assert!(
        work.content_keys <= 3 * 4,
        "empty-content deletion search inspected {} content keys for 3 shared contents and 257 placements",
        work.content_keys,
    );
    project.check_invariants().unwrap();
}

fn cleanup_fixture(unrelated: usize, auto_location: &str) -> Project {
    let template = fixture(2);
    let bar = template.meter().bar_ticks().unwrap();
    let mut edited = track(1000, 0);
    let mut clip = template.tracks()[0].clips[0].clone();
    clip.length = bar;
    clip.kind = if auto_location == "current" {
        ClipKind::Auto
    } else {
        ClipKind::Fixed
    };
    edited.clips.push(clip.clone());
    let mut fixed = track(1001, 1);
    clip.id = Id::from_raw(10_000);
    clip.kind = ClipKind::Fixed;
    clip.offset = Tick::new(-10);
    clip.gain = 0.5;
    clip.fade_in = 1;
    clip.fade_out = 2;
    fixed.clips.push(clip);
    let mut tracks = vec![edited, fixed];
    if auto_location == "other" {
        let mut owner = track(1002, 2);
        let mut clip = template.tracks()[0].clips[0].clone();
        clip.id = Id::from_raw(10_001);
        clip.kind = ClipKind::Auto;
        clip.length = bar;
        owner.clips.push(clip);
        tracks.push(owner);
    }
    for index in 0..unrelated {
        let mut other = track(2000 + index as u128, tracks.len() as u32);
        let mut clip = template.tracks()[0].clips[1].clone();
        clip.id = Id::from_raw(20_000 + index as u128);
        other.clips.push(clip);
        tracks.push(other);
    }
    Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        template
            .contents()
            .iter()
            .map(|content| (**content).clone())
            .collect(),
        tracks,
    )
    .unwrap()
}

#[test]
fn last_note_cleanup_copies_only_tracks_with_an_auto_placement_to_remove() {
    for count in [64, 256] {
        for location in ["current", "other", "none"] {
            let project = cleanup_fixture(count, location);
            let (deleted, work) = measured(|| {
                project
                    .delete_note(Id::from_raw(1000), Id::from_raw(2))
                    .unwrap()
            });
            assert_eq!(
                work.delete_track_copies,
                usize::from(location == "other"),
                "last-note cleanup copied {} tracks with auto placement {location} and {count} unrelated tracks",
                work.delete_track_copies,
            );
            assert!(deleted.content(Id::from_raw(1)).unwrap().notes.is_empty());
            assert_eq!(project.content(Id::from_raw(1)).unwrap().notes.len(), 1);
            for (before, after) in project.tracks().iter().zip(deleted.tracks()) {
                let mut expected = Track::clone(before);
                expected
                    .clips
                    .retain(|clip| clip.content != Id::from_raw(1) || clip.kind != ClipKind::Auto);
                assert_eq!(after.as_ref(), &expected);
                if before.id != Id::from_raw(1000) && before.clips == expected.clips {
                    assert!(Arc::ptr_eq(before, after));
                }
            }
            assert!(Arc::ptr_eq(&project.contents()[1], &deleted.contents()[1]));
            assert_roundtrip(&project, &deleted);
        }
    }
}

#[test]
fn last_note_cleanup_reclaims_content_only_without_fixed_links() {
    let project = cleanup_fixture(1, "current");
    let mut tracks: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| (**track).clone())
        .collect();
    tracks.remove(1);
    let project = Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        project
            .contents()
            .iter()
            .map(|content| (**content).clone())
            .collect(),
        tracks,
    )
    .unwrap();
    let deleted = project
        .delete_note(Id::from_raw(1000), Id::from_raw(2))
        .unwrap();
    assert!(deleted.tracks()[0].clips.is_empty());
    assert!(deleted.content(Id::from_raw(1)).is_none());
    assert!(Arc::ptr_eq(&project.contents()[1], &deleted.contents()[0]));
    assert_roundtrip(&project, &deleted);
}
