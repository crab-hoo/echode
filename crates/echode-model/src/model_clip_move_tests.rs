//! Clip-move ID collection budgets cover these fixtures, not product project limits.

use super::{fixture, measured};
use crate::{Id, Project, Tick};
use std::sync::Arc;

#[test]
fn moves_without_splits_do_not_collect_project_ids() {
    for count in [64, 256] {
        let project = fixture(count);
        let (project, empty) = project.add_track("empty destination");
        let (project, unrelated) = project.add_track("unrelated");
        let source_track = project.tracks()[0].id;
        let source = project.tracks()[0].clips[0].clone();
        let project = project.set_clip_gain(source.id, 0.5).unwrap();
        let project = project.set_clip_fades(source.id, 1, 2).unwrap();
        for (case, target, start, length) in [
            ("gap", source_track, count as i64 * 100, 100),
            ("empty track", empty, 0, 100),
            ("replace", source_track, 100, 100),
            ("trim left", source_track, 100, 20),
            ("trim right", source_track, 180, 20),
            ("own overlap", source_track, 10, 100),
        ] {
            let before = project
                .resize_clip(source_track, source.id, Tick::new(0), Tick::new(length))
                .unwrap();
            let original = before.clone();
            let old_clip = &before.tracks()[0].clips[0];
            for move_clip in [Project::move_clip, Project::move_clip_legacy] {
                let (result, work) = measured(|| {
                    move_clip(
                        &before,
                        source.id,
                        target,
                        Tick::new(start),
                        Id::from_raw(0),
                    )
                });
                let moved = result.unwrap();
                assert_eq!(
                    work.used_id_notes, 0,
                    "{case} collected {} note IDs with {count} independent contents",
                    work.used_id_notes,
                );
                assert_eq!(work.used_id_collections, 0, "{case} collected project IDs");
                moved.check_invariants().unwrap();
                let track = moved
                    .tracks()
                    .iter()
                    .find(|track| track.id == target)
                    .unwrap();
                let landed = track
                    .clips
                    .iter()
                    .find(|clip| clip.id == source.id)
                    .unwrap();
                assert_eq!(landed.start, Tick::new(start));
                assert_eq!(landed.length, old_clip.length);
                assert_eq!(landed.offset, old_clip.offset);
                assert_eq!(landed.content, old_clip.content);
                assert_eq!((landed.gain, landed.fade_in, landed.fade_out), (0.5, 1, 2));
                assert_eq!(before, original);
                for content in moved.contents() {
                    let old = before
                        .contents()
                        .iter()
                        .find(|old| old.id == content.id)
                        .unwrap();
                    assert!(Arc::ptr_eq(old, content));
                }
                let old = before
                    .tracks()
                    .iter()
                    .find(|track| track.id == unrelated)
                    .unwrap();
                let new = moved
                    .tracks()
                    .iter()
                    .find(|track| track.id == unrelated)
                    .unwrap();
                assert!(Arc::ptr_eq(old, new));
                assert_eq!(
                    before.apply_changeset(&before.diff(&moved)),
                    Ok(moved.clone())
                );
                assert_eq!(
                    moved.apply_changeset(&moved.diff(&before)),
                    Ok(before.clone())
                );
            }
        }
    }
}

#[test]
fn a_split_still_collects_ids_once_and_keeps_replayable_identity() {
    for count in [64, 256] {
        let project = fixture(count);
        let track = project.tracks()[0].id;
        let source = project.tracks()[0].clips[0].clone();
        let project = project
            .resize_clip(track, source.id, Tick::new(0), Tick::new(20))
            .unwrap();
        for move_clip in [Project::move_clip, Project::move_clip_legacy] {
            let (result, work) =
                measured(|| move_clip(&project, source.id, track, Tick::new(110), Id::from_raw(0)));
            let moved = result.unwrap();
            assert_eq!(work.used_id_collections, 1);
            assert_eq!(work.used_id_notes, count);
            moved.check_invariants().unwrap();
            assert_eq!(moved.tracks()[0].clips.len(), count + 1);
            assert_eq!(
                move_clip(&project, source.id, track, Tick::new(110), Id::from_raw(0)),
                Ok(moved)
            );
        }
    }
}
