//! Link retargeting and pruning scale with placements and contents, not their product.

#[path = "model_host_tests.rs"]
mod hosts;

use super::{fixture, measured};
use crate::{ClipKind, Id, ModelError, NoteDraft, NoteGeometry, Project, Tick, Track};
use std::sync::Arc;

const OWNER: u128 = 1_000_000;
const LINKED: u128 = 1_000_100;

fn merge_project(count: usize, owner_first: bool) -> Project {
    // Counts, IDs and timeline spacing bound this regression only.
    let template = fixture(count + 1);
    let bar = template.meter().bar_ticks().unwrap().get();
    let mut contents: Vec<_> = template
        .contents()
        .iter()
        .map(|item| (**item).clone())
        .collect();
    for content in &mut contents {
        content.notes[0].start = Tick::new(0);
        content.notes[0].length = Tick::new(1);
        content.notes[0].velocity = 40_001;
        content.notes[0].release = 32_001;
    }
    let track = |id, name: &str| Track {
        id: Id::from_raw(id),
        name: name.into(),
        parent: None,
        order: crate::initial_order(),
        clips: Vec::new(),
        devices: vec![crate::test_mixer(id)],
    };
    let mut owner = track(OWNER, "auto owner");
    for index in 0..count {
        let mut clip = template.tracks()[0].clips[index].clone();
        clip.start = Tick::new(index as i64 * 2 * bar);
        clip.length = Tick::new(bar);
        clip.kind = ClipKind::Auto;
        owner.clips.push(clip);
    }
    let mut linked = track(LINKED, "linked");
    let mut other = track(LINKED + 100, "other links");
    let mut unrelated = track(LINKED + 200, "unrelated");
    for index in 0..count {
        for (slot, target) in [&mut owner, &mut linked, &mut other, &mut unrelated]
            .into_iter()
            .enumerate()
        {
            let mut clip = template.tracks()[0].clips[count - 1 - index].clone();
            clip.id = Id::from_raw(2_000_000 + (slot * count + index) as u128);
            if slot == 3 {
                clip.content = contents[count].id;
            }
            clip.start = Tick::new((4 * count + index * 3) as i64 * bar);
            clip.length = Tick::new(2 * bar);
            clip.offset = Tick::new(-bar);
            clip.kind = ClipKind::Fixed;
            clip.gain = 0.25;
            clip.fade_in = 1;
            clip.fade_out = 2;
            target.clips.push(clip);
        }
    }
    let mut tracks = if owner_first {
        vec![owner, linked, other, unrelated]
    } else {
        vec![linked, other, unrelated, owner]
    };
    for (index, track) in tracks.iter_mut().enumerate() {
        track.order = crate::model::sibling_order(index as u32);
    }
    Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks).unwrap()
}

fn merge_edit(project: &Project, count: usize, operation: &str) -> Result<Project, ModelError> {
    let through_link = operation == "linked edit";
    let track = project
        .tracks()
        .iter()
        .find(|track| track.id == Id::from_raw(if through_link { LINKED } else { OWNER }))
        .unwrap();
    let clip = if through_link {
        track.clips.last().unwrap()
    } else {
        &track.clips[0]
    };
    let note = &project.content(clip.content).unwrap().notes[0];
    let start = clip.sounding_start(note).unwrap();
    let length = Tick::new(2 * count as i64 * project.meter().bar_ticks().unwrap().get());
    if operation == "insert" {
        project.insert_note_with_id(
            track.id,
            Id::from_raw(3_000_000),
            Id::from_raw(3_000_001),
            Id::from_raw(3_000_002),
            NoteDraft {
                pitch: 61,
                velocity: 100,
                start,
                length,
            },
        )
    } else {
        project.edit_note(
            track.id,
            clip.id,
            note.id,
            NoteGeometry {
                pitch: 61,
                start,
                length,
            },
        )
    }
}

fn assert_merged(before: &Project, after: &Project, count: usize, operation: &str) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    let primary = before.contents()[0].id;
    let bar = before.meter().bar_ticks().unwrap().get();
    let owner = after
        .tracks()
        .iter()
        .find(|track| track.id == Id::from_raw(OWNER))
        .unwrap();
    assert_eq!(owner.clips.len(), count + 1);
    assert_eq!(owner.clips[0].content, primary);
    assert_eq!(owner.clips[0].length, Tick::new(2 * count as i64 * bar));
    assert_eq!(after.contents().len(), 2);
    let merged = after.content(primary).unwrap();
    assert_eq!(
        merged.notes.len(),
        count + usize::from(operation == "insert")
    );
    for (index, content) in before.contents()[..count].iter().enumerate() {
        let original = &content.notes[0];
        let note = merged
            .notes
            .iter()
            .find(|note| note.id == original.id)
            .unwrap();
        let mut expected = original.clone();
        expected.start = Tick::new(index as i64 * 2 * bar);
        if index == 0 && operation != "insert" {
            expected.pitch = 61;
            expected.length = Tick::new(2 * count as i64 * bar);
        }
        assert_eq!(*note, expected);
        if index != 0 {
            assert!(after.content(content.id).is_none());
        }
    }
    for (old_track, new_track) in before.tracks().iter().zip(after.tracks()) {
        if old_track.name == "unrelated" {
            assert!(Arc::ptr_eq(old_track, new_track));
            continue;
        }
        for old in old_track
            .clips
            .iter()
            .filter(|clip| clip.kind == ClipKind::Fixed)
        {
            let new = new_track
                .clips
                .iter()
                .find(|clip| clip.id == old.id)
                .unwrap();
            let index = before
                .contents()
                .iter()
                .position(|content| content.id == old.content)
                .unwrap();
            let old_note = &before.content(old.content).unwrap().notes[0];
            let delta = index as i64 * 2 * bar - old_note.start.get();
            let mut expected = old.clone();
            expected.content = primary;
            expected.offset = Tick::new(old.offset.get() + delta);
            if operation == "linked edit" && old_track.id == Id::from_raw(LINKED) && index == 0 {
                expected.length = Tick::new((2 * count as i64 + 1) * bar);
            }
            assert_eq!(*new, expected);
            let new_note = merged
                .notes
                .iter()
                .find(|note| note.id == old_note.id)
                .unwrap();
            assert_eq!(old.sounding_start(old_note), new.sounding_start(new_note));
        }
    }
    let unrelated = before.contents().last().unwrap();
    assert!(Arc::ptr_eq(
        unrelated,
        after.shared_content(unrelated.id).unwrap()
    ));
    let diff = before.diff(after);
    assert!(
        before
            .apply_changeset(&diff)
            .unwrap()
            .diff(after)
            .is_empty()
    );
    assert!(
        after
            .apply_changeset(&diff.invert())
            .unwrap()
            .diff(before)
            .is_empty()
    );
}

#[test]
fn multi_clip_merge_retargets_links_and_prunes_contents_in_one_pass() {
    for count in [4, 64, 256] {
        for owner_first in [false, true] {
            let project = merge_project(count, owner_first);
            let placements: usize = project.tracks().iter().map(|track| track.clips.len()).sum();
            for operation in ["insert", "edit", "linked edit"] {
                let (edited, work) = measured(|| merge_edit(&project, count, operation).unwrap());
                assert_merged(&project, &edited, count, operation);
                assert!(
                    work.merge_placement_probes <= 4 * placements,
                    "multi-clip merge inspected {} placement keys for {placements} placements ({operation})",
                    work.merge_placement_probes
                );
                assert!(
                    work.merge_content_probes <= project.contents().len(),
                    "multi-clip merge inspected {} content keys for {} contents ({operation})",
                    work.merge_content_probes,
                    project.contents().len()
                );
                assert_eq!(
                    work.refit_track_copies,
                    if operation == "linked edit" { 3 } else { 2 }
                );
            }
        }
    }
}

#[test]
fn multi_clip_merge_discovery_visits_autos_at_most_twice() {
    // The counts bound this regression, not the number of product placements.
    for count in [64, 256] {
        let project = merge_project(count, true);
        for operation in ["insert", "edit", "linked edit"] {
            let (edited, work) = measured(|| merge_edit(&project, count, operation).unwrap());
            edited.check_invariants().unwrap();
            assert!(
                work.refit_host_probes <= 2 * count,
                "multi-clip merge searched {} host indices for {count} auto placements ({operation})",
                work.refit_host_probes
            );
        }
    }
}

#[test]
fn multi_clip_merge_wall_index_is_lazy_without_touching_autos() {
    // These fixture sizes bound only this regression, not product projects.
    for count in [1, 64, 256] {
        let project = merge_project(count, true);
        let clip = &project.tracks()[0].clips[0];
        let note = &project.content(clip.content).unwrap().notes[0];
        for operation in ["insert", "edit", "duplicate"] {
            let (edited, work) = measured(|| match operation {
                "insert" => project.insert_note_with_id(
                    Id::from_raw(OWNER),
                    Id::from_raw(3_000_000),
                    Id::from_raw(3_000_001),
                    Id::from_raw(3_000_002),
                    NoteDraft {
                        pitch: 61,
                        velocity: 100,
                        start: Tick::new(0),
                        length: Tick::new(1),
                    },
                ),
                "edit" => project.edit_note(
                    Id::from_raw(OWNER),
                    clip.id,
                    note.id,
                    NoteGeometry {
                        pitch: 61,
                        start: Tick::new(0),
                        length: Tick::new(1),
                    },
                ),
                _ => project.duplicate_note(
                    Id::from_raw(OWNER),
                    clip.id,
                    note.id,
                    Id::from_raw(3_000_000),
                    61,
                    Tick::new(0),
                ),
            });
            edited.unwrap().check_invariants().unwrap();
            assert_eq!(work.fixed_wall_probes, 0, "isolated auto {operation}");
        }
        let fixed = fixture(count);
        let bar = fixed.meter().bar_ticks().unwrap().get();
        let (edited, work) = measured(|| {
            fixed.insert_note_with_id(
                fixed.tracks()[0].id,
                Id::from_raw(3_000_000),
                Id::from_raw(3_000_001),
                Id::from_raw(3_000_002),
                NoteDraft {
                    pitch: 61,
                    velocity: 100,
                    start: Tick::new(2 * count as i64 * bar),
                    length: Tick::new(1),
                },
            )
        });
        edited.unwrap().check_invariants().unwrap();
        assert_eq!(work.fixed_wall_probes, 0, "no auto candidates");
    }
}

#[test]
fn multi_clip_merge_wall_queries_do_not_rescan_placements() {
    // Counts bound this regression only; fixed windows must not be scanned per candidate.
    for count in [64, 256] {
        let project = merge_project(count, true);
        let original = project.clone();
        let bar = project.meter().bar_ticks().unwrap().get();
        let placements = project.tracks()[0].clips.len();
        for operation in ["insert", "edit", "linked edit", "gap insert"] {
            let (edited, work) = measured(|| {
                if operation == "gap insert" {
                    project
                        .insert_note_with_id(
                            Id::from_raw(OWNER),
                            Id::from_raw(3_000_000),
                            Id::from_raw(3_000_001),
                            Id::from_raw(3_000_002),
                            NoteDraft {
                                pitch: 61,
                                velocity: 100,
                                start: Tick::new(bar),
                                length: Tick::new(2 * count as i64 * bar),
                            },
                        )
                        .unwrap()
                } else {
                    merge_edit(&project, count, operation).unwrap()
                }
            });
            assert!(
                work.fixed_wall_probes <= placements + 2 * count * (count.ilog2() as usize + 2),
                "merge wall lookup inspected {} placements for {count} auto placements ({operation})",
                work.fixed_wall_probes
            );
            edited.check_invariants().unwrap();
            assert!(project.diff(&original).is_empty());
            if operation == "gap insert" {
                assert_eq!(edited.tracks()[0].clips.len(), count + 1);
                let merged = edited.content(edited.tracks()[0].clips[0].content).unwrap();
                assert_eq!(merged.notes.len(), count + 1);
                assert_eq!(merged.notes[1].start, Tick::new(bar));
                let changes = project.diff(&edited);
                assert!(
                    project
                        .apply_changeset(&changes)
                        .unwrap()
                        .diff(&edited)
                        .is_empty()
                );
                assert!(
                    edited
                        .apply_changeset(&changes.invert())
                        .unwrap()
                        .diff(&project)
                        .is_empty()
                );
            } else {
                assert_merged(&project, &edited, count, operation);
            }
        }
    }
}

#[test]
fn multi_clip_merge_membership_keeps_reverse_chains_and_fixed_walls() {
    // Reverse chains must not rescan the whole track after each absorbed window.
    // These fixture sizes do not impose any limit on product projects.
    for count in [1, 4, 64, 256] {
        for wall in [false, true] {
            if wall && count == 1 {
                continue;
            }
            let project = hosts::reverse_project(count, wall, 0);
            let bar = project.meter().bar_ticks().unwrap().get();
            let original = project.clone();
            let clip = &project.tracks()[0].clips[count - 1];
            let note = &project.content(clip.content).unwrap().notes[0];
            let (edited, work) = measured(|| {
                project
                    .edit_note(
                        Id::from_raw(OWNER),
                        clip.id,
                        note.id,
                        NoteGeometry {
                            pitch: 61,
                            start: clip.sounding_start(note).unwrap(),
                            length: note.length,
                        },
                    )
                    .unwrap()
            });
            assert!(
                work.fixed_wall_probes
                    <= 2 * count + work.refit_host_probes * (count.ilog2() as usize + 2),
                "reverse merge wall lookup inspected {} placements for {count} clips (wall={wall})",
                work.fixed_wall_probes
            );
            let first = if wall { count / 2 + 1 } else { 0 };
            let merged_count = count - first;
            assert!(
                work.refit_host_probes <= 2 * count,
                "reverse merge searched {} host indices for {count} auto placements (wall={wall})",
                work.refit_host_probes
            );
            edited.check_invariants().unwrap();
            assert!(project.diff(&original).is_empty());
            let owner = &edited.tracks()[0];
            assert_eq!(owner.clips.len(), 2 * count - merged_count + 1);
            assert_eq!(owner.clips[..first], project.tracks()[0].clips[..first]);
            let primary = &owner.clips[first];
            assert_eq!(primary.id, project.tracks()[0].clips[first].id);
            assert_eq!(primary.start, Tick::new(first as i64 * bar));
            assert_eq!(primary.length, Tick::new(merged_count as i64 * bar));
            let merged = edited.content(primary.content).unwrap();
            assert_eq!(merged.notes.len(), merged_count);
            for (index, note) in merged.notes.iter().enumerate() {
                let old = &project.contents()[first + index].notes[0];
                assert_eq!(note.id, old.id);
                assert_eq!(note.start, Tick::new(index as i64 * bar));
                assert_eq!(note.velocity, old.velocity);
                assert_eq!(note.release, old.release);
                assert_eq!(
                    note.pitch,
                    if index + 1 == merged_count {
                        61
                    } else {
                        old.pitch
                    }
                );
            }
            let diff = project.diff(&edited);
            assert!(
                project
                    .apply_changeset(&diff)
                    .unwrap()
                    .diff(&edited)
                    .is_empty()
            );
            assert!(
                edited
                    .apply_changeset(&diff.invert())
                    .unwrap()
                    .diff(&project)
                    .is_empty()
            );
        }
    }
}

fn shifted_project() -> Project {
    let project = merge_project(4, true);
    let bar = project.meter().bar_ticks().unwrap().get();
    let mut contents: Vec<_> = project
        .contents()
        .iter()
        .map(|content| (**content).clone())
        .collect();
    let mut tracks: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| (**track).clone())
        .collect();
    let owner = tracks
        .iter_mut()
        .find(|track| track.id == Id::from_raw(OWNER))
        .unwrap();
    for (index, content) in contents.iter_mut().enumerate().take(4).skip(1) {
        let shift = if index % 2 == 0 { -16 * bar } else { 16 * bar };
        content.notes[0].start = Tick::new(shift);
        owner.clips[index].offset = Tick::new(shift);
    }
    Project::try_assemble(project.meter(), project.tempo().clone(), contents, tracks).unwrap()
}

#[test]
fn multi_clip_merge_translates_positive_and_negative_content_origins() {
    let project = shifted_project();
    for operation in ["insert", "edit", "linked edit"] {
        let edited = merge_edit(&project, 4, operation).unwrap();
        assert_merged(&project, &edited, 4, operation);
    }
}

#[test]
fn multi_clip_merge_rejects_link_offset_overflow_without_changing_original() {
    for upper in [false, true] {
        let project = shifted_project();
        let mut tracks: Vec<_> = project
            .tracks()
            .iter()
            .map(|track| (**track).clone())
            .collect();
        let bar = project.meter().bar_ticks().unwrap().get();
        let linked = tracks
            .iter_mut()
            .find(|track| track.id == Id::from_raw(LINKED))
            .unwrap();
        // Original origins and note times fit; the translated offsets do not.
        linked.clips = vec![linked.clips[usize::from(upper)].clone()];
        let boundary = if upper {
            i64::MAX - 4 * bar
        } else {
            i64::MIN + 4 * bar
        };
        linked.clips[0].start = Tick::new(boundary);
        linked.clips[0].offset = Tick::new(boundary);
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
        let original = project.clone();
        for operation in ["insert", "edit"] {
            assert_eq!(
                merge_edit(&project, 4, operation).unwrap_err(),
                ModelError::InvalidNote
            );
            assert!(project.diff(&original).is_empty());
            project.check_invariants().unwrap();
        }
    }
}
