//! Deterministic work counts for content lookup, reference validation and shared-note bounds (FCA-05).
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

pub(crate) fn note_validation() {
    record(|work| work.note_validations += 1);
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
