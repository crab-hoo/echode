//! Deterministic work counts for lookups, references, bounds, parents, refits, deletion, IDs and diffs.
//! Counts and fixture sizes bound these tests only, not product projects.

use std::cell::Cell;
use std::sync::Arc;

use crate::{ClipKind, Content, Id, NoteDraft, NoteGeometry, Project, Tick, Track};

#[derive(Clone, Copy, Default)]
struct Work {
    content_keys: usize,
    references: usize,
    overlaps: usize,
    note_validations: usize,
    parent_links: usize,
    shared_note_validations: usize,
    note_lookups: usize,
    refit_sync_probes: usize,
    refit_track_copies: usize,
    used_id_collections: usize,
    used_id_notes: usize,
    delete_track_copies: usize,
    id_checks: usize,
    id_check_notes: usize,
    diff_note_visits: usize,
    apply_device_owner_visits: usize,
    diff_device_keys: usize,
    diff_tempo_visits: usize,
    merge_placement_probes: usize,
    merge_content_probes: usize,
    refit_host_probes: usize,
    fixed_wall_probes: usize,
}

thread_local! {
    static WORK: Cell<Work> = Cell::new(Work::default());
}

fn record(update: impl FnOnce(&mut Work)) {
    WORK.with(|work| {
        let mut next = work.get();
        update(&mut next);
        work.set(next);
    });
}

pub(crate) fn content_key() {
    record(|work| work.content_keys += 1);
}

pub(crate) fn reference() {
    record(|work| work.references += 1);
}

pub(crate) fn overlap() {
    record(|work| work.overlaps += 1);
}

pub(crate) fn parent_link() {
    record(|work| work.parent_links += 1);
}

pub(crate) fn note_validation() {
    record(|work| work.note_validations += 1);
}

pub(crate) fn shared_note_validation() {
    record(|work| work.shared_note_validations += 1);
}

pub(crate) fn note_lookup() {
    record(|work| work.note_lookups += 1);
}

pub(crate) fn refit_sync_probe() {
    record(|work| work.refit_sync_probes += 1);
}

pub(crate) fn refit_track_copy() {
    record(|work| work.refit_track_copies += 1);
}

pub(crate) fn used_id_collection() {
    record(|work| work.used_id_collections += 1);
}

pub(crate) fn used_id_note() {
    record(|work| work.used_id_notes += 1);
}

pub(crate) fn delete_track_copy() {
    record(|work| work.delete_track_copies += 1);
}

pub(crate) fn id_check() {
    record(|work| work.id_checks += 1);
}

pub(crate) fn id_check_note() {
    record(|work| work.id_check_notes += 1);
}

pub(crate) fn diff_note_visit() {
    record(|work| work.diff_note_visits += 1);
}

pub(crate) fn apply_device_owner_visit() {
    record(|work| work.apply_device_owner_visits += 1);
}

pub(crate) fn diff_device_key() {
    record(|work| work.diff_device_keys += 1);
}

pub(crate) fn diff_tempo_visit() {
    record(|work| work.diff_tempo_visits += 1);
}

pub(crate) fn merge_placement_probe() {
    record(|work| work.merge_placement_probes += 1);
}

pub(crate) fn merge_content_probe() {
    record(|work| work.merge_content_probes += 1);
}

// Count both range collection and frontier visits, not only selected hosts.
pub(crate) fn refit_host_probe() {
    record(|work| work.refit_host_probes += 1);
}

pub(crate) fn fixed_wall_probe() {
    record(|work| work.fixed_wall_probes += 1);
}

fn measured<T>(run: impl FnOnce() -> T) -> (T, Work) {
    WORK.with(|work| work.set(Work::default()));
    let result = run();
    (result, WORK.with(Cell::get))
}

fn fixture(count: usize) -> Project {
    let (project, track) = Project::new().add_track("scale");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(10),
            },
        )
        .unwrap();
    let mut track = Track::clone(&project.tracks()[0]);
    let template = track.clips[0].clone();
    track.clips.clear();
    let mut contents = Vec::new();
    for index in 0..count {
        let mut content = Content::clone(&project.contents()[0]);
        content.id = Id::from_raw(index as u128 * 3 + 1);
        content.notes[0].id = Id::from_raw(index as u128 * 3 + 2);
        let mut clip = template.clone();
        clip.id = Id::from_raw(index as u128 * 3 + 3);
        clip.content = content.id;
        clip.start = Tick::new(index as i64 * 100);
        clip.length = Tick::new(100);
        clip.kind = ClipKind::Fixed;
        track.clips.push(clip);
        contents.push(content);
    }
    // Assembly accepts decoded content in any order and publishes it in ID order.
    contents.reverse();
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        vec![track],
    )
    .unwrap()
}

#[test]
fn content_lookup_uses_logarithmic_work_and_handles_missing_ids() {
    let project = fixture(256);
    let (_, work) = measured(|| {
        for content in project.contents() {
            assert_eq!(project.content(content.id), Some(content.as_ref()));
        }
        assert!(project.content(Id::from_raw(0)).is_none());
        assert!(project.content(Id::from_raw(1_000)).is_none());
        assert!(Project::new().content(Id::from_raw(1)).is_none());
    });
    assert!(
        work.content_keys <= 258 * 10,
        "content lookup inspected {} keys for 258 queries",
        work.content_keys
    );
}

#[test]
fn assembly_does_not_compare_every_content_or_pair_of_clips() {
    let project = fixture(256);
    let (_, work) = measured(|| {
        Project::try_assemble(
            project.meter(),
            project.tempo().clone(),
            project
                .contents()
                .iter()
                .map(|item| (**item).clone())
                .collect(),
            project
                .tracks()
                .iter()
                .map(|item| (**item).clone())
                .collect(),
        )
        .unwrap()
    });
    assert!(
        work.content_keys <= 256 * 20,
        "assembly inspected {} content keys for 256 clips",
        work.content_keys
    );
    assert!(
        work.overlaps <= 256,
        "assembly compared {} clip pairs for 256 clips",
        work.overlaps
    );
    assert!(
        work.references <= 256 * 2,
        "assembly inspected {} references for 256 clips",
        work.references
    );
}

#[test]
fn note_edit_does_not_rescan_placements_for_each_content() {
    for count in [64, 256] {
        let project = fixture(count);
        let track = &project.tracks()[0];
        let clip = &track.clips[0];
        let note = &project.content(clip.content).unwrap().notes[0];
        let (edited, work) = measured(|| {
            project
                .edit_note(
                    track.id,
                    clip.id,
                    note.id,
                    NoteGeometry {
                        pitch: 61,
                        start: note.start,
                        length: note.length,
                    },
                )
                .unwrap()
        });
        assert!(
            work.references <= count,
            "note edit inspected {} references for {count} placements",
            work.references
        );
        edited.check_invariants().unwrap();
        assert_eq!(project.content(clip.content).unwrap().notes[0].pitch, 60);
        assert_eq!(edited.content(clip.content).unwrap().notes[0].pitch, 61);
        for (old, new) in project.contents()[1..].iter().zip(&edited.contents()[1..]) {
            assert!(Arc::ptr_eq(old, new));
        }
    }
}

#[test]
fn content_id_order_is_an_invariant() {
    let project = fixture(2);
    let mut contents: Vec<_> = project
        .contents()
        .iter()
        .map(|item| (**item).clone())
        .collect();
    contents.reverse();
    let broken = Project::unchecked(
        project.meter(),
        project.tempo().clone(),
        contents,
        project
            .tracks()
            .iter()
            .map(|item| (**item).clone())
            .collect(),
    );
    assert!(
        broken
            .check_invariants()
            .is_err_and(|error| error.to_string().contains("content id order")),
        "unsorted content id order was accepted"
    );
}

#[test]
fn reclamation_keeps_shared_content_until_its_last_placement_is_gone() {
    let project = fixture(2);
    let (project, _) = project.add_track("linked");
    let mut contents = project.contents().to_vec();
    let mut tracks = project.tracks().to_vec();
    let mut linked = tracks[0].clips[0].clone();
    linked.id = Id::from_raw(10_000);
    Arc::make_mut(&mut tracks[1]).clips.push(linked);
    Arc::make_mut(&mut tracks[0]).clips.clear();
    crate::placement::drop_unreferenced(&mut contents, &tracks);
    assert_eq!(contents.len(), 1);
    assert!(Arc::ptr_eq(&contents[0], &project.contents()[0]));
    Arc::make_mut(&mut tracks[1]).clips.clear();
    crate::placement::drop_unreferenced(&mut contents, &tracks);
    assert!(contents.is_empty());
    project.check_invariants().unwrap();
}

fn shared_fixture(notes: usize, placements: usize) -> (Project, Vec<Content>, Vec<Track>) {
    let template = fixture(1);
    let mut content = Content::clone(&template.contents()[0]);
    let note = content.notes[0].clone();
    content.notes = (0..notes)
        .map(|index| {
            let mut note = note.clone();
            note.id = Id::from_raw(20_000 + index as u128);
            note.start = Tick::new(index as i64 * 10 - 100);
            note.length = Tick::new((notes - index) as i64 * 20);
            note
        })
        .collect();
    let mut track = Track::clone(&template.tracks()[0]);
    let clip = track.clips[0].clone();
    track.clips = (0..placements)
        .map(|index| {
            let mut clip = clip.clone();
            clip.id = Id::from_raw(10_000 + index as u128);
            clip.start = Tick::new(index as i64 * 100);
            clip
        })
        .collect();
    (template, vec![content], vec![track])
}

#[test]
fn shared_content_validation_does_not_rescan_notes_per_placement() {
    // These sizes constrain work in this regression, not product projects.
    for (notes, placements) in [(64, 256), (256, 64)] {
        let (template, contents, tracks) = shared_fixture(notes, placements);
        let (project, assembly) = measured(|| {
            Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks)
                .unwrap()
        });
        let (_, validation) = measured(|| project.check_invariants().unwrap());
        assert!(
            assembly.note_validations <= notes * 2 && validation.note_validations <= notes,
            "shared content validation visited notes {} times during assembly and {} times during validation for {notes} notes and {placements} placements",
            assembly.note_validations,
            validation.note_validations,
        );
    }
}

#[test]
fn shared_content_edits_do_not_rescan_notes_per_placement() {
    // These sizes constrain this regression, not product notes or placements.
    for (notes, placements) in [(64, 256), (256, 64)] {
        let (template, contents, mut tracks) = shared_fixture(notes, placements);
        let (template, _) = template.add_track("linked");
        let mut linked = Track::clone(&template.tracks()[1]);
        linked.clips = tracks[0].clips.split_off(placements / 2);
        tracks.push(linked);
        let project =
            Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks)
                .unwrap();
        let track = &project.tracks()[0];
        let clip = &track.clips[0];
        let content = project.content(clip.content).unwrap();
        let note = &content.notes[0];
        for operation in ["edit", "insert", "duplicate"] {
            let (edited, work) = measured(|| match operation {
                "edit" => project.edit_note(
                    track.id,
                    clip.id,
                    note.id,
                    NoteGeometry {
                        pitch: 61,
                        start: note.start,
                        length: note.length,
                    },
                ),
                "insert" => project
                    .insert_note(
                        track.id,
                        NoteDraft {
                            pitch: 62,
                            velocity: 100,
                            start: Tick::new(1),
                            length: Tick::new(1),
                        },
                    )
                    .map(|(project, _)| project),
                "duplicate" => project.duplicate_note(
                    track.id,
                    clip.id,
                    content.notes.last().unwrap().id,
                    Id::from_raw(30_000),
                    63,
                    Tick::new(1),
                ),
                _ => unreachable!(),
            });
            let edited = edited.unwrap();
            assert!(
                work.shared_note_validations > 0 && work.shared_note_validations <= notes + 1,
                "shared content {operation} visited notes {} times for {notes} notes and {placements} placements",
                work.shared_note_validations,
            );
            edited.check_invariants().unwrap();
            assert!(Arc::ptr_eq(&project.tracks()[1], &edited.tracks()[1]));
            assert_eq!(project.content(clip.content).unwrap().notes.len(), notes);
            assert_eq!(project.content(clip.content).unwrap().notes[0].pitch, 60);
            let expected_notes = if operation == "edit" {
                notes
            } else {
                notes + 1
            };
            assert_eq!(
                edited.content(clip.content).unwrap().notes.len(),
                expected_notes
            );
        }
    }
}

fn nested_tracks(count: usize) -> Vec<Track> {
    (0..count)
        .map(|index| {
            let id = index as u128 + 1;
            Track {
                id: Id::from_raw(id),
                name: format!("level {index}"),
                parent: index
                    .checked_sub(1)
                    .map(|parent| Id::from_raw(parent as u128 + 1)),
                order: crate::initial_order(),
                devices: vec![crate::test_mixer(id)],
                clips: Vec::new(),
            }
        })
        .collect()
}

#[test]
fn nested_forest_validation_does_not_revisit_resolved_ancestors() {
    // These depths constrain this regression, not product folder nesting.
    for count in [64, 256] {
        let template = Project::new();
        let (project, assembly) = measured(|| {
            Project::try_assemble(
                template.meter(),
                template.tempo().clone(),
                Vec::new(),
                nested_tracks(count),
            )
            .unwrap()
        });
        let (_, validation) = measured(|| project.check_invariants().unwrap());
        assert!(
            assembly.parent_links <= count * 2 && validation.parent_links <= count,
            "nested forest validation inspected parent links {} times during assembly and {} times during validation for {count} tracks",
            assembly.parent_links,
            validation.parent_links,
        );
    }
}

#[test]
fn reverse_ordered_forest_validation_is_linear_before_order_rejection() {
    // These depths bound only this regression; unordered input must not
    // trigger repeated ancestor walks before its arrange order is rejected.
    for count in [64, 256] {
        let template = Project::new();
        let mut tracks = nested_tracks(count);
        tracks.reverse();
        let broken = Project::unchecked(
            template.meter(),
            template.tempo().clone(),
            Vec::new(),
            tracks.clone(),
        );
        let (error, assembly) = measured(|| {
            Project::try_assemble(
                template.meter(),
                template.tempo().clone(),
                Vec::new(),
                tracks,
            )
            .unwrap_err()
        });
        assert_eq!(error, crate::ModelError::BadOrder);
        let (error, validation) = measured(|| broken.check_invariants().unwrap_err());
        assert!(error.to_string().contains("out of arrange order"));
        assert!(
            assembly.parent_links <= count && validation.parent_links <= count,
            "reverse forest validation inspected parent links {} times during assembly and {} times during validation for {count} tracks",
            assembly.parent_links,
            validation.parent_links,
        );
    }
}

fn reparent_fixture(depth: usize) -> (Project, Id, Id) {
    let template = Project::new();
    let mut chain = nested_tracks(depth);
    chain[0].order = crate::model::sibling_order(1);
    let source = Id::from_raw(depth as u128 + 1);
    let child = Id::from_raw(depth as u128 + 2);
    let mut tracks: Vec<_> = [(source, None), (child, Some(source))]
        .into_iter()
        .map(|(id, parent)| Track {
            id,
            name: format!("moving {id}"),
            parent,
            order: crate::initial_order(),
            devices: vec![crate::test_mixer(id.raw())],
            clips: Vec::new(),
        })
        .collect();
    tracks.extend(chain);
    let project = Project::try_assemble(
        template.meter(),
        template.tempo().clone(),
        Vec::new(),
        tracks,
    )
    .unwrap();
    (project, source, child)
}

#[test]
fn nested_reparent_does_not_search_all_tracks_for_each_ancestor() {
    // These depths bound this regression only, not product folder nesting.
    for depth in [64, 256] {
        let (project, source, child) = reparent_fixture(depth);
        let destination = Id::from_raw(depth as u128);
        let (moved, work) = measured(|| project.set_parent(source, Some(destination)).unwrap());
        assert!(
            work.parent_links <= project.tracks().len(),
            "nested reparent inspected parent links {} times for {} tracks",
            work.parent_links,
            project.tracks().len(),
        );
        moved.check_invariants().unwrap();
        project.check_invariants().unwrap();
        assert_eq!(project.parent(source).unwrap(), None);
        assert_eq!(moved.parent(source).unwrap(), Some(destination));
        assert_eq!(moved.parent(child).unwrap(), Some(source));
        let expected: Vec<_> = (1..=depth)
            .map(|id| Id::from_raw(id as u128))
            .chain([source, child])
            .collect();
        assert_eq!(
            moved
                .tracks()
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>(),
            expected,
        );
        for original in project.tracks().iter().filter(|track| track.id != source) {
            let retained = moved
                .tracks()
                .iter()
                .find(|track| track.id == original.id)
                .unwrap();
            assert!(Arc::ptr_eq(original, retained));
        }
        let (unchanged, work) = measured(|| moved.set_parent(source, Some(destination)).unwrap());
        assert_eq!(work.parent_links, 0, "unchanged parent walked ancestors");
        for (old, new) in moved.tracks().iter().zip(unchanged.tracks()) {
            assert!(Arc::ptr_eq(old, new));
        }
        let detached = moved.set_parent(source, None).unwrap();
        detached.check_invariants().unwrap();
        assert_eq!(detached.parent(source).unwrap(), None);
        assert_eq!(detached.parent(child).unwrap(), Some(source));
    }
}

#[test]
fn nested_reparent_rejects_cycles_without_repeated_ancestor_searches() {
    // These depths bound this regression only, not product folder nesting.
    for depth in [64, 256] {
        let (project, source, child) = reparent_fixture(depth);
        let root = Id::from_raw(1);
        let leaf = Id::from_raw(depth as u128);
        for (track, parent) in [(root, leaf), (root, root), (source, child)] {
            let (error, work) = measured(|| project.set_parent(track, Some(parent)).unwrap_err());
            assert_eq!(error, crate::ModelError::Cycle);
            assert!(
                work.parent_links <= project.tracks().len(),
                "nested reparent cycle check inspected parent links {} times for {} tracks",
                work.parent_links,
                project.tracks().len(),
            );
        }
        let missing = Id::from_raw(0);
        assert_eq!(
            project.set_parent(source, Some(missing)).unwrap_err(),
            crate::ModelError::MissingTrack,
        );
        assert_eq!(
            project.set_parent(missing, Some(leaf)).unwrap_err(),
            crate::ModelError::MissingTrack,
        );
        project.check_invariants().unwrap();
    }
}

#[test]
fn note_edits_do_not_search_unrelated_contents_after_refitting() {
    // These sizes bound this regression only, not product notes or contents.
    for count in [64, 256] {
        let project = fixture(count);
        let track = &project.tracks()[0];
        let clip = track.clips.last().unwrap();
        let note = &project.content(clip.content).unwrap().notes[0];
        let start = clip.sounding_start(note).unwrap();
        for operation in ["edit", "insert", "duplicate"] {
            let (edited, work) = measured(|| match operation {
                "edit" => project.edit_note(
                    track.id,
                    clip.id,
                    note.id,
                    NoteGeometry {
                        pitch: 61,
                        start,
                        length: note.length,
                    },
                ),
                "insert" => project.insert_note_with_id(
                    track.id,
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
                "duplicate" => project.duplicate_note(
                    track.id,
                    clip.id,
                    note.id,
                    Id::from_raw(40_000),
                    61,
                    start,
                ),
                _ => unreachable!(),
            });
            let edited = edited.unwrap();
            let expected_lookups = match operation {
                "edit" => 1,
                "insert" => 0,
                "duplicate" => 2,
                _ => unreachable!(),
            };
            assert_eq!(
                work.note_lookups, expected_lookups,
                "{operation} searched {} notes with {count} independent contents",
                work.note_lookups,
            );
            edited.check_invariants().unwrap();
            assert_eq!(project.content(clip.content).unwrap().notes.len(), 1);
            assert_eq!(project.content(clip.content).unwrap().notes[0].pitch, 60);
            assert!(
                edited
                    .content(clip.content)
                    .unwrap()
                    .notes
                    .iter()
                    .any(|note| note.pitch == 61)
            );
            for (old, new) in project.contents()[..count - 1]
                .iter()
                .zip(&edited.contents()[..count - 1])
            {
                assert!(Arc::ptr_eq(old, new));
            }
        }
    }
}

#[path = "model_diff_tests.rs"]
mod diff;

#[path = "model_refit_tests.rs"]
mod refit;

#[path = "model_clip_move_tests.rs"]
mod clip_move;

#[path = "model_delete_tests.rs"]
mod delete;

#[path = "model_insert_tests.rs"]
mod insert;

#[path = "model_merge_tests.rs"]
mod merge;

#[path = "model_device_diff_tests.rs"]
mod device_diff;

#[path = "model_tempo_diff_tests.rs"]
mod tempo_diff;

#[path = "model_device_apply_tests.rs"]
mod device_apply;
