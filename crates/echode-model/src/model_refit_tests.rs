//! Linked edits must not copy unrelated tracks or synchronize windows quadratically.

use super::{fixture, measured};
use crate::{ClipKind, Id, ModelError, NoteDraft, NoteGeometry, Project, Tick, Track};
use std::sync::Arc;

fn linked_project(count: usize, auto_first: bool, has_auto: bool) -> Project {
    // Sizes and IDs bound this regression only, not product tracks or placements.
    let template = fixture(3);
    let bar = template.meter().bar_ticks().unwrap().get();
    let mut contents: Vec<_> = template
        .contents()
        .iter()
        .map(|item| (**item).clone())
        .collect();
    for content in &mut contents {
        content.notes[0].length = Tick::new(1);
    }
    let track = |id: u128| Track {
        id: Id::from_raw(id),
        name: format!("refit {id}"),
        parent: None,
        order: crate::initial_order(),
        clips: Vec::new(),
        devices: vec![crate::test_mixer(id)],
    };
    let mut linked = track(1000);
    for index in 0..count {
        let mut clip = template.tracks()[0].clips[index % 3].clone();
        clip.id = Id::from_raw(10_000 + (count - index) as u128);
        // Alternate retired, surviving, and unrelated contents in timeline order.
        clip.content = contents[[1, 0, 2][index % 3]].id;
        clip.start = Tick::new(index as i64 * 3 * bar);
        clip.length = Tick::new(2 * bar);
        clip.offset = Tick::new(-bar);
        clip.gain = 0.25 * (1 + index % 3) as f32;
        clip.fade_in = 1;
        clip.fade_out = 2;
        linked.clips.push(clip);
    }
    let mut tracks = vec![linked];
    for index in 0..count {
        let mut unrelated = track(2000 + index as u128);
        let mut clip = template.tracks()[0].clips[2].clone();
        clip.id = Id::from_raw(20_000 + index as u128);
        unrelated.clips.push(clip);
        tracks.push(unrelated);
    }
    let mut owner = track(3000);
    for (index, content) in contents[..2].iter().enumerate() {
        let mut clip = template.tracks()[0].clips[index].clone();
        clip.id = Id::from_raw(30_000 + index as u128);
        clip.content = content.id;
        clip.start = Tick::new(index as i64 * 2 * bar);
        clip.length = Tick::new(bar);
        clip.offset = Tick::new(0);
        clip.kind = if has_auto {
            ClipKind::Auto
        } else {
            ClipKind::Fixed
        };
        owner.clips.push(clip);
    }
    if auto_first {
        tracks.insert(0, owner);
    } else {
        tracks.push(owner);
    }
    for (index, track) in tracks.iter_mut().enumerate() {
        track.order = crate::model::sibling_order(index as u32);
    }
    Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks).unwrap()
}

fn edit_link(project: &Project, operation: &str, merge: bool) -> Result<Project, ModelError> {
    let linked = project
        .tracks()
        .iter()
        .find(|track| track.id == Id::from_raw(1000))
        .unwrap();
    let clip = &linked.clips[0];
    let note = &project.content(clip.content).unwrap().notes[0];
    let bar = project.meter().bar_ticks().unwrap().get();
    let start = Tick::new(if merge { 0 } else { bar });
    match operation {
        "edit" => project.edit_note(
            linked.id,
            clip.id,
            note.id,
            NoteGeometry {
                pitch: 61,
                start,
                length: note.length,
            },
        ),
        "insert" => project.insert_note_with_id(
            linked.id,
            Id::from_raw(40_000),
            Id::from_raw(40_001),
            Id::from_raw(40_002),
            NoteDraft {
                pitch: 61,
                velocity: 100,
                start,
                length: Tick::new(1),
            },
        ),
        "duplicate" => {
            project.duplicate_note(linked.id, clip.id, note.id, Id::from_raw(40_000), 61, start)
        }
        _ => unreachable!(),
    }
}

fn assert_linked_edit(before: &Project, after: &Project, merged: bool, earlier: bool) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let bar = before.meter().bar_ticks().unwrap().get();
    let surviving = before.contents()[0].id;
    let retired = before.contents()[1].id;
    let original = before
        .tracks()
        .iter()
        .find(|track| track.id == Id::from_raw(1000))
        .unwrap();
    let edited = after
        .tracks()
        .iter()
        .find(|track| track.id == original.id)
        .unwrap();
    assert_eq!(original.clips.len(), edited.clips.len());
    for (old, new) in original.clips.iter().zip(&edited.clips) {
        let mut expected = old.clone();
        if merged && old.content == retired {
            expected.content = surviving;
            expected.offset = Tick::new(old.offset.get() + 2 * bar);
        }
        assert_eq!(
            *new, expected,
            "fixed window {} changed unexpectedly",
            old.id
        );
        let content = after.content(new.content).unwrap();
        if old.content == retired {
            let note = content.notes.iter().find(|note| note.pitch == 61).unwrap();
            assert_eq!(
                new.sounding_start(note).unwrap(),
                Tick::new(old.start.get() + if earlier { 0 } else { bar })
            );
        }
    }
    assert_eq!(before.contents().len(), 3);
    assert_eq!(before.content(retired).unwrap().notes[0].pitch, 60);
    if merged {
        assert!(after.content(retired).is_none());
        assert_eq!(after.contents().len(), 2);
    }
    for original in before
        .tracks()
        .iter()
        .filter(|track| (2000..3000).contains(&track.id.raw()))
    {
        let retained = after
            .tracks()
            .iter()
            .find(|track| track.id == original.id)
            .unwrap();
        assert!(Arc::ptr_eq(original, retained));
    }
    let unrelated = before.contents().last().unwrap();
    assert!(Arc::ptr_eq(
        unrelated,
        after.shared_content(unrelated.id).unwrap()
    ));
    let diff = before.diff(after);
    let replayed = before.apply_changeset(&diff).unwrap();
    assert!(replayed.diff(after).is_empty());
    assert!(
        after
            .apply_changeset(&diff.invert())
            .unwrap()
            .diff(before)
            .is_empty()
    );
}

#[test]
fn linked_refit_synchronization_is_linear_in_tracks_and_placements() {
    for count in [1, 64, 256] {
        for auto_first in [false, true] {
            let project = linked_project(count, auto_first, true);
            for merge in [false, true] {
                for operation in ["edit", "insert", "duplicate"] {
                    let (edited, work) =
                        measured(|| edit_link(&project, operation, merge).unwrap());
                    assert!(
                        work.refit_sync_probes <= project.tracks().len() + 2 * count,
                        "linked refit inspected {} synchronization keys for {count} placements ({operation})",
                        work.refit_sync_probes,
                    );
                    assert_linked_edit(&project, &edited, merge, merge);
                }
            }
        }
    }
}

#[test]
fn linked_refit_does_not_copy_unrelated_tracks() {
    for has_auto in [false, true] {
        for merge in [false, true] {
            let project = linked_project(64, false, has_auto);
            let expected = if !has_auto {
                0
            } else if merge {
                2
            } else {
                1
            };
            for operation in ["edit", "insert", "duplicate"] {
                let (edited, work) = measured(|| edit_link(&project, operation, merge).unwrap());
                assert_eq!(
                    work.refit_track_copies, expected,
                    "linked refit copied {} tracks (auto={has_auto}, merge={merge})",
                    work.refit_track_copies,
                );
                assert_linked_edit(&project, &edited, has_auto && merge, merge);
            }
        }
    }
}

#[test]
fn linked_refit_deletion_keeps_work_bounded_and_fixed_windows_unchanged() {
    for has_auto in [false, true] {
        let project = linked_project(64, false, has_auto);
        let project = edit_link(&project, "insert", false).unwrap();
        let (edited, work) = measured(|| {
            project
                .delete_note(Id::from_raw(1000), Id::from_raw(40_000))
                .unwrap()
        });
        assert_eq!(work.refit_track_copies, usize::from(has_auto));
        assert!(work.refit_sync_probes <= project.tracks().len() + 2 * 64);
        edited.check_invariants().unwrap();
        project.check_invariants().unwrap();
        for (before, after) in project.tracks().iter().zip(edited.tracks()) {
            assert_eq!(before.clips, after.clips);
            if before.id != Id::from_raw(1000) && before.id != Id::from_raw(3000) {
                assert!(Arc::ptr_eq(before, after));
            }
        }
        assert_eq!(project.contents()[1].notes.len(), 2);
        assert_eq!(edited.contents()[1].notes.len(), 1);
    }
}
