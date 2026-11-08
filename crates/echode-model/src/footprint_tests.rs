use std::mem::size_of;

use crate::{Id, MixerIds, Note, NoteDraft, Project, Tempo, Tick};

/// Three tracks; the first holds one clip with `notes` notes in its first bar.
fn project_with_notes(notes: u8) -> (Project, Id) {
    let (project, first) = Project::new().add_track("first");
    let (project, _) = project.add_track("second");
    let (mut project, _) = project.add_track("third");
    for index in 0..notes {
        let draft = NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::new(i64::from(index) * 10),
            length: Tick::new(10),
        };
        project = project.insert_note(first, draft).unwrap().0;
    }
    assert_eq!(project.contents().len(), usize::from(notes > 0));
    (project, first)
}

#[test]
fn a_whole_version_counts_every_object() {
    let (project, _) = project_with_notes(4);
    let footprint = project.footprint();
    assert_eq!(footprint.tracks, 3);
    assert_eq!(footprint.contents, 1);
    assert_eq!(footprint.notes, 4);
    assert!(footprint.bytes >= 4 * size_of::<Note>() as u64);
}

#[test]
fn an_identical_version_shares_every_object() {
    let (project, _) = project_with_notes(4);
    let footprint = project.footprint_beyond(&project.clone());
    assert_eq!(
        (footprint.tracks, footprint.contents, footprint.notes),
        (0, 0, 0)
    );
    // Only the version's own lists remain, and they do not grow with notes.
    let (larger, _) = project_with_notes(40);
    assert_eq!(
        footprint.bytes,
        larger.footprint_beyond(&larger.clone()).bytes
    );
}

#[test]
fn a_note_edit_keeps_only_the_replaced_content() {
    let (before, first) = project_with_notes(8);
    let clip = before.tracks()[0].clips[0].id;
    let note = before.contents()[0].notes[3].id;
    let after = before.set_note_velocity(first, clip, note, 1).unwrap();
    for (older, newer) in [(&before, &after), (&after, &before)] {
        let kept = older.footprint_beyond(newer);
        assert_eq!((kept.tracks, kept.contents, kept.notes), (0, 1, 8));
        assert!(kept.bytes >= 8 * size_of::<Note>() as u64);
    }
}

#[test]
fn a_shifted_track_order_still_matches_by_id() {
    let (before, first) = project_with_notes(2);
    let after = before
        .insert_track_after_with_id(
            Id::generate(),
            first,
            false,
            "inserted",
            MixerIds::generate(),
        )
        .unwrap();
    assert_eq!(after.tracks()[1].name, "inserted");
    let kept = before.footprint_beyond(&after);
    assert_eq!((kept.tracks, kept.contents), (0, 0));
    let added = after.footprint_beyond(&before);
    assert_eq!((added.tracks, added.contents), (1, 0));
}

#[test]
fn a_tempo_edit_counts_the_replaced_points_once() {
    let (before, _) = project_with_notes(1);
    let tempo = Tempo::quarters_per_minute(90).unwrap();
    let after = before.with_tempo(Tick::new(960), tempo);
    let shared = before.footprint_beyond(&before.clone()).bytes;
    let kept = before.footprint_beyond(&after);
    assert_eq!((kept.tracks, kept.contents), (0, 0));
    assert!(kept.bytes > shared);
}
