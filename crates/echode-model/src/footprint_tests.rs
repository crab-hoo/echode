use std::mem::size_of;

use crate::{ClipKind, Id, Lane, MixerIds, Note, NoteDraft, Placement, Project, Tempo, Tick};

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
fn a_note_edit_keeps_only_the_replaced_content_and_run() {
    let (before, first) = project_with_notes(8);
    let clip = before.tracks()[0].clips[0].id;
    let note = before.contents()[0].notes[3].id;
    let after = before.set_note_velocity(first, clip, note, 1).unwrap();
    for (older, newer) in [(&before, &after), (&after, &before)] {
        let kept = older.footprint_beyond(newer);
        let run = run_holding(older, note);
        assert_eq!((kept.tracks, kept.contents, kept.notes), (0, 1, run as u64));
        assert!(kept.bytes >= (run * size_of::<Note>()) as u64);
    }
}

fn run_holding(project: &Project, note: Id) -> usize {
    project.contents()[0]
        .notes
        .runs()
        .iter()
        .find(|run| run.iter().any(|item| item.id == note))
        .map_or(0, |run| run.len())
}

/// One track with one clip of `count` notes in its first bar. Ids count up,
/// so the runs are the same on every run of the test.
fn project_with_many_notes(count: u128) -> (Project, Id) {
    let (project, track) = project_with_notes(1);
    let mut content = crate::Content::clone(&project.contents()[0]);
    let template = content.notes[0].clone();
    let mut notes: Vec<Note> = (0..count)
        .map(|index| Note {
            id: Id::from_raw(1_000_000 + index),
            start: Tick::new((index % 3_000) as i64),
            pitch: (index % 128) as u8,
            ..template.clone()
        })
        .collect();
    crate::model::sort_notes(&mut notes);
    content.notes = notes.into();
    let tracks = project
        .tracks()
        .iter()
        .map(|track| crate::Track::clone(track))
        .collect();
    let project = Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        vec![content],
        tracks,
    )
    .unwrap();
    (project, track)
}

#[test]
fn a_one_note_edit_of_ten_thousand_notes_keeps_a_few_runs_not_the_content() {
    // FCA-19: one undo step of this edit held all 10,000 notes (801 KB). The
    // count fixes the scenario; it is not a product limit.
    let (before, track) = project_with_many_notes(10_000);
    let clip = before.tracks()[0].clips[0].id;
    let target = before.contents()[0].notes[5_000].clone();
    let fresh = Id::from_raw(9_000_000);
    let edits: Vec<(&str, Project)> = vec![
        (
            "velocity",
            before.set_note_velocity(track, clip, target.id, 1).unwrap(),
        ),
        (
            "move",
            before
                .edit_note(
                    track,
                    clip,
                    target.id,
                    crate::NoteGeometry {
                        pitch: 1,
                        start: Tick::new(7),
                        length: target.length,
                    },
                )
                .unwrap(),
        ),
        (
            "duplicate",
            before
                .duplicate_note(track, clip, target.id, fresh, 2, Tick::new(2_999))
                .unwrap(),
        ),
        ("delete", before.delete_note(track, target.id).unwrap()),
    ];
    let longest = before.contents()[0]
        .notes
        .runs()
        .iter()
        .map(|run| run.len())
        .max()
        .unwrap();
    assert!(longest < 1_000, "runs of up to {longest} notes");
    for (name, after) in edits {
        after.check_invariants().unwrap();
        for (older, newer) in [(&before, &after), (&after, &before)] {
            let kept = older.footprint_beyond(newer);
            // A note edit may also refit the clip, which replaces its track.
            assert!(kept.tracks <= 1 && kept.contents == 1, "{name}: {kept:?}");
            assert!(
                kept.notes as usize <= 3 * (longest + 1),
                "{name} kept {} of 10,000 notes",
                kept.notes
            );
            // The run list itself stays: about 16 bytes for each of ~156 runs.
            assert!(kept.bytes < 64 * 1_024, "{name} kept {} bytes", kept.bytes);
        }
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

/// One track of `count` fixed one-bar clips two bars apart, each with its own
/// one-note content. Ids count up, so the runs are the same on every run.
fn project_with_many_clips(count: u128) -> (Project, Id, i64) {
    let (project, track) = Project::new().add_track("sliced");
    let bar = project.meter().bar_ticks().unwrap().get();
    let mut contents = Vec::new();
    let mut clips = Vec::new();
    for slot in 0..count {
        let content = Id::from_raw(2_000_000 + slot);
        contents.push(crate::Content {
            id: content,
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(3_000_000 + slot),
                start: Tick::new(0),
                length: Tick::new(10),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            }]
            .into(),
            expressions: Vec::new(),
        });
        clips.push(Placement {
            id: Id::from_raw(1_000_000 + slot),
            content,
            lane: Lane::Timeline,
            start: Tick::new(slot as i64 * 2 * bar),
            length: Tick::new(bar),
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
        });
    }
    let mut sliced = crate::Track::clone(&project.tracks()[0]);
    sliced.clips = clips.into();
    let project = Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        contents,
        vec![sliced],
    )
    .unwrap();
    (project, track, bar)
}

#[test]
fn a_one_clip_edit_of_two_thousand_clips_keeps_a_few_runs_not_the_track() {
    // ADR 0353: one undo step of a clip edit held every placement of the
    // track (176 KB at 2,000 clips). The count fixes the scenario; it is not a
    // product limit.
    let (before, track, bar) = project_with_many_clips(2_000);
    let clips = &before.tracks()[0].clips;
    let target = clips[1_000].clone();
    let gap = Tick::new(target.start.get() + bar);
    let edits: Vec<(&str, Project)> = vec![
        (
            "resize",
            before
                .resize_clip(track, target.id, target.start, Tick::new(bar / 2))
                .unwrap(),
        ),
        (
            "move",
            before
                .move_clip(target.id, track, gap, Id::from_raw(9_000_000))
                .unwrap(),
        ),
        (
            "link",
            before
                .link_clip(target.id, track, gap, Id::from_raw(9_000_001))
                .unwrap(),
        ),
        (
            "duplicate",
            before
                .duplicate_clip(target.id, track, gap, Id::from_raw(9_000_002))
                .unwrap(),
        ),
        (
            "delete note",
            before
                .delete_note(track, before.content(target.content).unwrap().notes[0].id)
                .unwrap(),
        ),
    ];
    let longest = clips.runs().iter().map(|run| run.len()).max().unwrap();
    assert!(longest < 1_000, "runs of up to {longest} placements");
    for (name, after) in edits {
        after.check_invariants().unwrap();
        for (older, newer) in [(&before, &after), (&after, &before)] {
            let kept = older.footprint_beyond(newer);
            assert!(kept.tracks == 1 && kept.contents <= 1, "{name}: {kept:?}");
            assert!(
                kept.placements as usize <= 3 * (longest + 1),
                "{name} kept {} of 2,000 placements",
                kept.placements
            );
            // Every version still owns its content list: 8 bytes a content.
            assert!(kept.bytes < 64 * 1_024, "{name} kept {} bytes", kept.bytes);
        }
    }
}
