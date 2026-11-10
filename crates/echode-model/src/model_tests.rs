use super::*;
use crate::id::Id;
use crate::time::{Meter, TICKS_PER_QUARTER, Tempo, TempoMap, Tick};
use std::sync::Arc;

#[test]
fn midi_velocities_round_trip_through_sixteen_bits() {
    assert_eq!(midi7_to_velocity(127), 65535);
    assert_eq!(midi7_to_velocity(0), 0);
    for midi in 0..=127 {
        assert_eq!(velocity_to_midi7(midi7_to_velocity(midi)), midi);
    }
}

fn at(start_quarters: i64, length_quarters: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(start_quarters).unwrap(),
        length: Tick::quarters(length_quarters).unwrap(),
    }
}

fn project_with_track() -> (Project, Id) {
    Project::new().add_track("piano")
}

#[test]
fn clip_gain_stops_at_the_finite_mix_ceiling() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip = only_clip(&project).id;
    assert!(project.set_clip_gain(clip, CLIP_GAIN_MAX).is_ok());
    assert_eq!(
        project.set_clip_gain(clip, f32::MAX),
        Err(ModelError::InvalidGain)
    );
    assert!(project.set_clip_gain(clip, f32::NAN).is_err());
    assert!(clip_gain_allowed(1_000.0));
}

fn only_clip(project: &Project) -> &Placement {
    assert_eq!(project.tracks()[0].clips.len(), 1);
    &project.tracks()[0].clips[0]
}

fn first_note<'a>(project: &'a Project, clip: &Placement) -> &'a Note {
    &project.content(clip.content).expect("content").notes[0]
}

fn sounding(project: &Project, clip: &Placement) -> Tick {
    clip.sounding_start(first_note(project, clip)).unwrap()
}

#[test]
fn placing_a_note_creates_one_bar_auto_clip() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(1, 1)).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.kind, ClipKind::Auto);
    assert_eq!(clip.start.get(), 0);
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
    assert_eq!(sounding(&project, clip), Tick::quarters(1).unwrap());
}

#[test]
fn the_next_bar_extends_an_auto_clip_without_moving_notes() {
    let (project, track) = project_with_track();
    let (project, first) = project.insert_note(track, at(0, 1)).unwrap();
    let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 8);
    let kept = project
        .content(clip.content)
        .unwrap()
        .notes
        .iter()
        .find(|note| note.id == first)
        .unwrap();
    assert_eq!(clip.sounding_start(kept).unwrap().get(), 0);
}

#[test]
fn a_gap_starts_a_second_clip_until_the_gap_is_filled() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
    assert_eq!(project.tracks()[0].clips.len(), 2);
    let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
    let clip = only_clip(&project);
    assert_eq!(project.content(clip.content).unwrap().notes.len(), 3);
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 12);
}

#[test]
fn deleting_notes_shrinks_and_then_removes_an_auto_clip() {
    let (project, track) = project_with_track();
    let (project, early) = project.insert_note(track, at(0, 1)).unwrap();
    let (project, late) = project.insert_note(track, at(4, 1)).unwrap();
    let project = project.delete_note(track, late).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
    assert_eq!(sounding(&project, clip).get(), 0);
    let project = project.delete_note(track, early).unwrap();
    assert!(project.tracks()[0].clips.is_empty());
}

#[test]
fn resizing_fixes_the_bounds_and_a_neighbor_starts_a_new_clip() {
    let (project, track) = project_with_track();
    let (project, note) = project.insert_note(track, at(0, 1)).unwrap();
    let clip_id = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip_id, Tick::new(0), Tick::quarters(4).unwrap())
        .unwrap();
    assert_eq!(only_clip(&project).kind, ClipKind::Fixed);
    let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
    assert_eq!(project.tracks()[0].clips.len(), 2);
    assert_eq!(project.tracks()[0].clips[0].kind, ClipKind::Fixed);
    assert_eq!(project.tracks()[0].clips[1].kind, ClipKind::Auto);
    let fixed = &project.tracks()[0].clips[0];
    assert_eq!(sounding(&project, fixed).get(), 0);
    assert_eq!(first_note(&project, fixed).id, note);
}

#[test]
fn a_note_inside_a_fixed_clip_does_not_resize_it() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip_id = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip_id, Tick::new(0), Tick::quarters(8).unwrap())
        .unwrap();
    let (project, _) = project.insert_note(track, at(5, 1)).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.kind, ClipKind::Fixed);
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 8);
    assert_eq!(project.content(clip.content).unwrap().notes.len(), 2);
}

#[test]
fn an_auto_clip_stops_at_a_following_fixed_clip() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
    let later = project.tracks()[0].clips[1].id;
    let project = project
        .resize_clip(
            track,
            later,
            Tick::quarters(8).unwrap(),
            Tick::quarters(4).unwrap(),
        )
        .unwrap();
    let (project, _) = project.insert_note(track, at(4, 1)).unwrap();
    assert_eq!(project.tracks()[0].clips.len(), 2);
    assert_eq!(project.tracks()[0].clips[0].kind, ClipKind::Auto);
    assert_eq!(
        project.tracks()[0].clips[0].length.get(),
        TICKS_PER_QUARTER * 8
    );
    assert_eq!(project.tracks()[0].clips[1].kind, ClipKind::Fixed);
    assert_eq!(
        project.tracks()[0].clips[1].start,
        Tick::quarters(8).unwrap()
    );
}

#[test]
fn a_note_that_runs_through_a_fixed_clip_is_rejected() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let (project, _) = project.insert_note(track, at(8, 1)).unwrap();
    let later = project.tracks()[0].clips[1].id;
    let project = project
        .resize_clip(
            track,
            later,
            Tick::quarters(8).unwrap(),
            Tick::quarters(4).unwrap(),
        )
        .unwrap();
    let spanning = NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(4).unwrap(),
        length: Tick::quarters(5).unwrap(),
    };
    assert_eq!(
        project.insert_note(track, spanning).unwrap_err(),
        ModelError::Overlap
    );
    assert_eq!(project.tracks()[0].clips.len(), 2);
}

#[test]
fn three_four_uses_a_three_quarter_bar() {
    let meter = Meter::try_new(3, 4).unwrap();
    let project = Project::new().with_meter(meter).unwrap();
    let (project, track) = project.add_track("piano");
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    assert_eq!(only_clip(&project).length.get(), TICKS_PER_QUARTER * 3);
    let (project, _) = project.insert_note(track, at(3, 1)).unwrap();
    assert_eq!(only_clip(&project).length.get(), TICKS_PER_QUARTER * 6);
}

#[test]
fn a_note_before_the_origin_occupies_the_previous_bar() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(-1, 1)).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.start, Tick::quarters(-4).unwrap());
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
    assert_eq!(sounding(&project, clip), Tick::quarters(-1).unwrap());
}

#[test]
fn edits_keep_the_previous_snapshot_and_share_other_tracks() {
    let (project, edited) = project_with_track();
    let (project, other) = project.add_track("drums");
    let shared = project
        .tracks()
        .iter()
        .find(|track| track.id == other)
        .unwrap();
    let (next, _) = project.insert_note(edited, at(0, 1)).unwrap();
    assert!(project.tracks()[0].clips.is_empty());
    assert_eq!(
        next.tracks()
            .iter()
            .find(|track| track.id == edited)
            .unwrap()
            .clips
            .len(),
        1
    );
    let still = next
        .tracks()
        .iter()
        .find(|track| track.id == other)
        .unwrap();
    assert!(Arc::ptr_eq(shared, still));
    assert!(next.delete_note(edited, Id::generate()).is_err());
}

#[test]
fn invalid_notes_and_a_locked_meter_are_rejected() {
    let (project, track) = project_with_track();
    let mut draft = at(0, 1);
    draft.pitch = 128;
    assert_eq!(
        project.insert_note(track, draft).unwrap_err(),
        ModelError::InvalidNote
    );
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    assert_eq!(
        project
            .with_meter(Meter::try_new(3, 4).unwrap())
            .unwrap_err(),
        ModelError::InvalidMeter
    );
}

#[test]
fn assembled_auto_clips_fit_their_notes() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let content = Content {
        id: Id::from_raw(5),
        audio: None,
        takes: Vec::new(),
        notes: vec![Note {
            id: Id::from_raw(3),
            start: Tick::new(0),
            length: Tick::quarters(1).unwrap(),
            pitch: 60,
            velocity: 100,
            release: 0,
            articulation: None,
        }]
        .into(),
        expressions: Vec::new(),
    };
    let project = Project::try_assemble(
        Meter::four_four(),
        tempo,
        vec![content],
        vec![Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: sibling_order(0),
            devices: vec![crate::device::test_mixer(1)],
            clips: vec![Placement::new_auto(
                Id::from_raw(2),
                Id::from_raw(5),
                Tick::new(0),
                Tick::quarters(4).unwrap(),
            )],
        }],
    )
    .unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.id, Id::from_raw(2));
    assert_eq!(clip.length.get(), TICKS_PER_QUARTER * 4);
    assert_eq!(sounding(&project, clip).get(), 0);
}

#[test]
fn assembled_clips_refuse_overlap_and_duplicate_ids() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let clip = |id, content, start| Placement {
        id: Id::from_raw(id),
        content: Id::from_raw(content),
        lane: Lane::Timeline,
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(4).unwrap(),
        offset: Tick::new(0),
        kind: ClipKind::Fixed,
        gain: 1.0,
        fade_in: DEFAULT_FADE_FLICKS,
        fade_out: DEFAULT_FADE_FLICKS,
    };
    let shell = |id| Content {
        id: Id::from_raw(id),
        audio: None,
        takes: Vec::new(),
        notes: Vec::new().into(),
        expressions: Vec::new(),
    };
    let error = Project::try_assemble(
        Meter::four_four(),
        tempo.clone(),
        vec![shell(20), shell(30)],
        vec![Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: sibling_order(0),
            devices: vec![crate::device::test_mixer(1)],
            clips: vec![clip(2, 20, 0), clip(3, 30, 0)],
        }],
    )
    .unwrap_err();
    assert_eq!(error, ModelError::Overlap);
    let error = Project::try_assemble(
        Meter::four_four(),
        tempo,
        vec![shell(20)],
        vec![Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: sibling_order(0),
            devices: vec![crate::device::test_mixer(1)],
            clips: vec![clip(1, 20, 0)],
        }],
    )
    .unwrap_err();
    assert_eq!(error, ModelError::DuplicateId);
}

#[test]
fn caller_chosen_ids_are_kept_and_reused_ids_are_rejected() {
    let (project, track) = project_with_track();
    let note = Id::from_raw(1);
    let clip = Id::from_raw(2);
    let project = project
        .insert_note_with_id(track, note, clip, Id::from_raw(8), at(0, 1))
        .unwrap();
    let stored = only_clip(&project);
    assert_eq!(stored.id, clip);
    assert_eq!(first_note(&project, stored).id, note);

    let joined = project
        .insert_note_with_id(track, Id::from_raw(4), clip, Id::from_raw(9), at(1, 1))
        .unwrap();
    assert_eq!(
        joined
            .content(only_clip(&joined).content)
            .unwrap()
            .notes
            .len(),
        2
    );

    assert_eq!(
        project
            .insert_note_with_id(track, note, Id::from_raw(3), Id::from_raw(10), at(1, 1))
            .unwrap_err(),
        ModelError::DuplicateId
    );
    assert_eq!(
        project
            .insert_note_with_id(track, Id::from_raw(5), clip, Id::from_raw(11), at(8, 1))
            .unwrap_err(),
        ModelError::DuplicateId
    );
}

#[test]
fn resizing_preserves_timeline_position_when_the_start_moves() {
    let (project, track) = project_with_track();
    let (project, note) = project.insert_note(track, at(1, 1)).unwrap();
    let clip_id = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(
            track,
            clip_id,
            Tick::quarters(1).unwrap(),
            Tick::quarters(4).unwrap(),
        )
        .unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.kind, ClipKind::Fixed);
    let kept = project
        .content(clip.content)
        .unwrap()
        .notes
        .iter()
        .find(|item| item.id == note)
        .unwrap();
    assert_eq!(
        clip.sounding_start(kept).unwrap(),
        Tick::quarters(1).unwrap()
    );
}

#[test]
fn a_child_is_placed_after_the_folders_subtree() {
    let (project, folder) = Project::new().add_track("drums");
    let (project, later) = project.add_track("vox");
    let (project, kick) = project.add_child(folder, "kick").unwrap();
    let (project, room) = project.add_child(kick, "room").unwrap();
    let (project, snare) = project.add_child(folder, "snare").unwrap();
    let ids: Vec<_> = project.tracks().iter().map(|track| track.id).collect();
    assert_eq!(ids, vec![folder, kick, room, snare, later]);
    assert_eq!(project.children(folder).unwrap(), vec![kick, snare]);
    assert_eq!(project.parent(room).unwrap(), Some(kick));
    assert_eq!(project.parent(later).unwrap(), None);
}

#[test]
fn a_track_cannot_move_into_its_own_subtree() {
    let (project, folder) = Project::new().add_track("drums");
    let (project, kick) = project.add_child(folder, "kick").unwrap();
    assert_eq!(
        project.set_parent(folder, Some(kick)).unwrap_err(),
        ModelError::Cycle
    );
    assert_eq!(
        project.set_parent(folder, Some(folder)).unwrap_err(),
        ModelError::Cycle
    );
    assert_eq!(
        project.add_child(Id::from_raw(9), "missing").unwrap_err(),
        ModelError::MissingTrack
    );
    let project = project.set_parent(kick, None).unwrap();
    assert_eq!(project.parent(kick).unwrap(), None);
    assert_eq!(project.tracks()[1].id, kick);
}

#[test]
fn saved_folders_reject_a_cycle_and_a_missing_parent() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let bare = |id: u128, parent: Option<u128>| Track {
        id: Id::from_raw(id),
        name: "track".into(),
        parent: parent.map(Id::from_raw),
        order: sibling_order(id as u32),
        devices: vec![crate::device::test_mixer(id)],
        clips: Vec::new(),
    };
    let error = Project::try_assemble(
        Meter::four_four(),
        tempo.clone(),
        Vec::new(),
        vec![bare(1, Some(2)), bare(2, Some(1))],
    )
    .unwrap_err();
    assert_eq!(error, ModelError::Cycle);
    let error = Project::try_assemble(
        Meter::four_four(),
        tempo,
        Vec::new(),
        vec![bare(1, Some(9))],
    )
    .unwrap_err();
    assert_eq!(error, ModelError::MissingTrack);
}

/// 64 is only how far this test walks. The model does not cap folder depth.
#[test]
fn a_folder_can_nest_without_a_fixed_depth_cap() {
    let (mut project, mut parent) = Project::new().add_track("root");
    for level in 0..64 {
        let (next, child) = project.add_child(parent, format!("level {level}")).unwrap();
        project = next;
        parent = child;
    }
    assert_eq!(project.tracks().len(), 65);
    let mut cursor = project.parent(parent).unwrap();
    let mut depth = 0;
    while cursor.is_some() {
        depth += 1;
        cursor = project.parent(cursor.unwrap()).unwrap();
    }
    assert_eq!(depth, 64);
}

#[test]
fn a_note_added_through_one_placement_is_visible_from_the_other() {
    let (project, piano) = project_with_track();
    let (project, drums) = project.add_track("drums");
    let (project, first) = project.insert_note(piano, at(0, 1)).unwrap();
    let source = only_clip(&project).id;
    let project = project
        .resize_clip(piano, source, Tick::new(0), Tick::quarters(4).unwrap())
        .unwrap();
    let project = project
        .link_clip(source, drums, Tick::quarters(16).unwrap(), Id::from_raw(9))
        .unwrap();
    let (project, second) = project.insert_note(piano, at(1, 1)).unwrap();
    let piano_clip = project
        .tracks()
        .iter()
        .find(|track| track.id == piano)
        .unwrap()
        .clips
        .first()
        .unwrap();
    let drum_clip = project
        .tracks()
        .iter()
        .find(|track| track.id == drums)
        .unwrap()
        .clips
        .first()
        .unwrap();
    assert_eq!(piano_clip.kind, ClipKind::Fixed);
    assert_eq!(drum_clip.kind, ClipKind::Fixed);
    assert_eq!(piano_clip.content, drum_clip.content);
    let notes = &project.content(drum_clip.content).unwrap().notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].id, first);
    assert_eq!(notes[0].start.get(), 0);
    assert_eq!(notes[1].id, second);
    assert_eq!(
        piano_clip.sounding_start(&notes[1]).unwrap(),
        Tick::quarters(1).unwrap()
    );
    assert_eq!(
        drum_clip.sounding_start(&notes[1]).unwrap(),
        Tick::quarters(17).unwrap()
    );
    project.check_invariants().unwrap();
}

fn window(
    id: u128,
    content: u128,
    start: i64,
    length: i64,
    offset: i64,
    kind: ClipKind,
) -> Placement {
    Placement {
        id: Id::from_raw(id),
        content: Id::from_raw(content),
        lane: Lane::Timeline,
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
        offset: Tick::quarters(offset).unwrap(),
        kind,
        gain: 1.0,
        fade_in: DEFAULT_FADE_FLICKS,
        fade_out: DEFAULT_FADE_FLICKS,
    }
}

fn content_with(id: u128, note: u128, start: i64) -> Content {
    Content {
        id: Id::from_raw(id),
        audio: None,
        takes: Vec::new(),
        notes: vec![Note {
            id: Id::from_raw(note),
            start: Tick::quarters(start).unwrap(),
            length: Tick::quarters(1).unwrap(),
            pitch: 60,
            velocity: 100,
            release: 0,
            articulation: None,
        }]
        .into(),
        expressions: Vec::new(),
    }
}

fn shared_across_tracks(blocker: bool) -> (Project, Id, Id) {
    let mut drum_clips = Vec::new();
    let mut contents = vec![content_with(5, 3, 0)];
    if blocker {
        contents.push(content_with(8, 11, 0));
        drum_clips.push(window(7, 8, -4, 4, 0, ClipKind::Fixed));
    }
    drum_clips.push(window(6, 5, 0, 4, 0, ClipKind::Auto));
    let project = Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        contents,
        vec![
            Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                order: sibling_order(0),
                devices: vec![crate::device::test_mixer(1)],
                clips: vec![window(2, 5, -4, 8, -4, ClipKind::Fixed)],
            },
            Track {
                id: Id::from_raw(4),
                name: "drums".into(),
                parent: None,
                order: sibling_order(1),
                devices: vec![crate::device::test_mixer(4)],
                clips: drum_clips,
            },
        ],
    )
    .unwrap();
    project.check_invariants().unwrap();
    (project, Id::from_raw(1), Id::from_raw(3))
}

#[test]
fn an_earlier_note_grows_only_the_auto_on_the_other_track() {
    let (project, piano, kept) = shared_across_tracks(false);
    let (project, _) = project.insert_note(piano, at(-1, 1)).unwrap();
    let content = project.content(Id::from_raw(5)).unwrap();
    let kept_note = content.notes.iter().find(|note| note.id == kept).unwrap();
    assert_eq!(kept_note.start.get(), 0);
    let added = content.notes.iter().find(|note| note.id != kept).unwrap();
    assert_eq!(added.start, Tick::quarters(-1).unwrap());
    let piano_clip = &project.tracks()[0].clips[0];
    assert_eq!(piano_clip.kind, ClipKind::Fixed);
    assert_eq!(piano_clip.start, Tick::quarters(-4).unwrap());
    assert_eq!(piano_clip.length, Tick::quarters(8).unwrap());
    assert_eq!(piano_clip.offset, Tick::quarters(-4).unwrap());
    let auto = &project.tracks()[1].clips[0];
    assert_eq!(auto.kind, ClipKind::Auto);
    assert_eq!(auto.start, Tick::quarters(-4).unwrap());
    assert_eq!(auto.length, Tick::quarters(8).unwrap());
    assert_eq!(auto.offset, Tick::quarters(-4).unwrap());
    assert_eq!(auto.sounding_start(kept_note).unwrap(), Tick::new(0));
    project.check_invariants().unwrap();
}

#[test]
fn growing_the_auto_on_the_other_track_rejects_an_overlap() {
    let (project, piano, _) = shared_across_tracks(true);
    assert_eq!(
        project.insert_note(piano, at(-1, 1)).unwrap_err(),
        ModelError::Overlap
    );
    assert_eq!(project.tracks()[1].clips[1].start.get(), 0);
    assert_eq!(
        project.content(Id::from_raw(5)).unwrap().notes[0]
            .start
            .get(),
        0
    );
}

fn fade_past_u32() -> i64 {
    let rate = i128::from(FADE_RATE_LIMIT);
    let frames = i128::from(u32::MAX) + 1;
    let numer = frames * i128::from(crate::time::FLICKS_PER_SECOND);
    i64::try_from((numer + rate - 1) / rate).unwrap()
}

#[test]
fn a_fade_that_does_not_fit_in_u32_frames_is_rejected() {
    assert_eq!(fade_frames(0, 44_100), Some(0));
    assert_eq!(fade_frames(1, 44_100), Some(0));
    assert_eq!(fade_frames(DEFAULT_FADE_FLICKS, 44_100), Some(220));
    let too_wide = fade_past_u32();
    assert!(fade_frames(too_wide, FADE_RATE_LIMIT).is_none());
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip = only_clip(&project).id;
    assert_eq!(
        project.set_clip_fades(clip, too_wide, 0).unwrap_err(),
        ModelError::InvalidFade
    );
    let mut assembled = (*project.tracks()[0]).clone();
    assembled.clips[0].fade_in = too_wide;
    assert_eq!(
        Project::try_assemble(
            project.meter(),
            project.tempo().clone(),
            project
                .contents()
                .iter()
                .map(|content| (**content).clone())
                .collect(),
            vec![assembled],
        )
        .unwrap_err(),
        ModelError::InvalidFade
    );
}

#[test]
fn fades_longer_than_the_placement_are_accepted() {
    let (project, track) = project_with_track();
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip = only_clip(&project).id;
    // Four quarters at 120 bpm last two seconds. Each fade is sixty seconds.
    let fade = crate::time::FLICKS_PER_SECOND * 60;
    let project = project.set_clip_fades(clip, fade, fade).unwrap();
    let clip = only_clip(&project);
    assert_eq!(clip.length, Tick::quarters(4).unwrap());
    assert_eq!(clip.fade_in, fade);
    assert_eq!(clip.fade_out, fade);
    project.check_invariants().unwrap();
}

#[test]
fn an_earlier_note_is_stored_before_the_one_already_there() {
    let (project, track) = project_with_track();
    let (project, later) = project.insert_note(track, at(4, 1)).unwrap();
    let kept = first_note(&project, only_clip(&project)).start;
    let (project, earlier) = project.insert_note(track, at(0, 1)).unwrap();
    let notes = &project.content(only_clip(&project).content).unwrap().notes;
    assert_eq!(notes[0].id, earlier);
    assert_eq!(notes[1].id, later);
    assert_eq!(notes[1].start, kept);
    project.check_invariants().unwrap();
}

#[test]
fn the_lower_pitch_comes_first_when_notes_start_together() {
    let (project, track) = project_with_track();
    let high = NoteDraft {
        pitch: 67,
        ..at(0, 1)
    };
    let low = NoteDraft {
        pitch: 60,
        ..at(0, 1)
    };
    let (project, high_id) = project.insert_note(track, high).unwrap();
    let (project, low_id) = project.insert_note(track, low).unwrap();
    let notes = &project.content(only_clip(&project).content).unwrap().notes;
    assert_eq!(notes[0].id, low_id);
    assert_eq!(notes[1].id, high_id);
    assert_eq!(notes[0].start, notes[1].start);
    assert_eq!(notes[1].pitch, 67);
    project.check_invariants().unwrap();
}

#[test]
fn the_smaller_id_comes_first_when_start_and_pitch_match() {
    let (project, track) = project_with_track();
    let project = project
        .insert_note_with_id(
            track,
            Id::from_raw(20),
            Id::from_raw(2),
            Id::from_raw(9),
            at(0, 1),
        )
        .unwrap();
    let project = project
        .insert_note_with_id(
            track,
            Id::from_raw(10),
            Id::from_raw(3),
            Id::from_raw(11),
            at(0, 1),
        )
        .unwrap();
    let notes = &project.content(only_clip(&project).content).unwrap().notes;
    assert_eq!(notes[0].id, Id::from_raw(10));
    assert_eq!(notes[1].id, Id::from_raw(20));
    assert_eq!(notes[0].start, notes[1].start);
    assert_eq!(notes[0].pitch, notes[1].pitch);
    project.check_invariants().unwrap();
}

#[test]
fn unsorted_notes_are_rejected() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let content = Content {
        id: Id::from_raw(5),
        audio: None,
        takes: Vec::new(),
        notes: vec![
            Note {
                id: Id::from_raw(4),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 67,
                velocity: 100,
                release: 0,
                articulation: None,
            },
            Note {
                id: Id::from_raw(3),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: 100,
                release: 0,
                articulation: None,
            },
        ]
        .into(),
        expressions: Vec::new(),
    };
    let track = Track {
        id: Id::from_raw(1),
        name: "piano".into(),
        parent: None,
        order: sibling_order(0),
        devices: vec![crate::device::test_mixer(1)],
        clips: vec![Placement::new_auto(
            Id::from_raw(2),
            Id::from_raw(5),
            Tick::new(0),
            Tick::quarters(4).unwrap(),
        )],
    };
    assert_eq!(
        Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            vec![content.clone()],
            vec![track.clone()]
        )
        .unwrap_err(),
        ModelError::Unsorted
    );
    let stored = Project::unchecked(Meter::four_four(), tempo, vec![content], vec![track]);
    assert!(
        stored
            .check_invariants()
            .unwrap_err()
            .to_string()
            .contains("out of start, pitch, id order")
    );
}

#[test]
fn moving_a_track_under_another_keeps_every_other_order_key() {
    use crate::changeset::{Change, ObjectKey, Property};
    let (project, a) = Project::new().add_track("A");
    let (project, b) = project.add_track("B");
    let (project, _) = project.add_child(a, "a1").unwrap();
    let before = project.clone();
    let orders: Vec<(Id, String)> = project
        .tracks()
        .iter()
        .map(|track| (track.id, track.order.clone()))
        .collect();
    let project = project.set_parent(a, Some(b)).unwrap();
    let names: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| track.name.as_str())
        .collect();
    assert_eq!(names, ["B", "A", "a1"]);
    for track in project.tracks() {
        if track.id != a {
            let (_, previous) = orders.iter().find(|(id, _)| *id == track.id).unwrap();
            assert_eq!(&track.order, previous);
        }
    }
    let changes = before.diff(&project);
    assert!(changes.changes().iter().all(|change| {
        !matches!(
            change,
            Change::Update {
                property: Property::TrackOrder,
                ..
            }
        )
    }));
    let order_ids: Vec<Id> = changes
        .changes()
        .iter()
        .filter_map(|change| match change {
            Change::Update {
                id: ObjectKey::Id(id),
                property: Property::Order,
                ..
            } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(order_ids.iter().all(|id| *id == a));
    project.check_invariants().unwrap();
}

#[test]
fn a_bad_track_order_is_rejected() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let track = |id: u128, order: String| Track {
        id: Id::from_raw(id),
        name: format!("t{id}"),
        parent: None,
        order,
        devices: vec![crate::device::test_mixer(id)],
        clips: Vec::new(),
    };
    let same = sibling_order(0);
    assert_eq!(
        Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            Vec::new(),
            vec![track(1, same.clone()), track(2, same)],
        )
        .unwrap_err(),
        ModelError::Order(Box::new(crate::ArrangeError::DuplicateKey {
            first: Id::from_raw(1),
            first_name: "t1".into(),
            second: Id::from_raw(2),
            second_name: "t2".into(),
        }))
    );
    assert_eq!(
        Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            Vec::new(),
            vec![track(1, "not-a-key".into())],
        )
        .unwrap_err(),
        ModelError::Order(Box::new(crate::ArrangeError::InvalidKey {
            track: Id::from_raw(1),
            name: "t1".into(),
        }))
    );
    let swapped = vec![track(1, sibling_order(1)), track(2, sibling_order(0))];
    assert_eq!(
        Project::try_assemble(
            Meter::four_four(),
            tempo.clone(),
            Vec::new(),
            swapped.clone()
        )
        .unwrap_err(),
        ModelError::Order(Box::new(crate::ArrangeError::OutOfOrder {
            track: Id::from_raw(1),
            name: "t1".into(),
        }))
    );
    let stored = Project::unchecked(Meter::four_four(), tempo, Vec::new(), swapped);
    assert!(
        stored
            .check_invariants()
            .unwrap_err()
            .to_string()
            .contains("out of arrange order")
    );
}

fn fixed_window(start: i64, length: i64, offset: i64) -> Placement {
    Placement {
        id: Id::from_raw(1),
        content: Id::from_raw(2),
        lane: Lane::Timeline,
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
        offset: Tick::quarters(offset).unwrap(),
        kind: ClipKind::Fixed,
        gain: 1.0,
        fade_in: DEFAULT_FADE_FLICKS,
        fade_out: DEFAULT_FADE_FLICKS,
    }
}

fn tone(start: i64, length: i64) -> Note {
    Note {
        id: Id::from_raw(3),
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
        pitch: 60,
        velocity: 100,
        release: 0,
        articulation: None,
    }
}

#[test]
fn a_window_sounds_only_where_it_overlaps_the_note() {
    let note = tone(0, 4);
    // Content time 0 is the timeline origin when offset equals the window start.
    assert_eq!(fixed_window(2, 2, 2).audible(&tone(0, 1)).unwrap(), None);
    assert_eq!(
        fixed_window(0, 1, 0).audible(&note).unwrap(),
        Some((Tick::new(0), Tick::quarters(1).unwrap()))
    );
    assert_eq!(
        fixed_window(1, 1, 1).audible(&note).unwrap(),
        Some((Tick::quarters(1).unwrap(), Tick::quarters(2).unwrap()))
    );
    assert_eq!(fixed_window(0, 1, 0).audible(&tone(1, 1)).unwrap(), None);
    assert_eq!(
        fixed_window(0, 1, 0).audible(&tone(0, 1)).unwrap(),
        Some((Tick::new(0), Tick::quarters(1).unwrap()))
    );
    assert_eq!(fixed_window(5, 1, 5).audible(&note).unwrap(), None);
    assert!(fixed_window(0, 1, 0).audible(&note).unwrap().is_some());
}

#[test]
fn audit_pro13_a_link_at_the_time_edge_is_refused_and_the_project_stays() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let resized = project
        .resize_clip(track, clip, Tick::new(i64::MIN), Tick::new(1))
        .unwrap();
    let linked = resized.link_clip(clip, track, Tick::new(0), Id::from_raw(900));
    assert!(linked.is_err());
    assert_eq!(resized.tracks()[0].clips.len(), 1);

    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, at(0, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let resized = project
        .resize_clip(track, clip, Tick::new(0), Tick::new(1))
        .unwrap();
    let linked = resized.link_clip(clip, track, Tick::new(i64::MAX - 1), Id::from_raw(901));
    assert!(linked.is_err());
    assert_eq!(resized.tracks()[0].clips.len(), 1);
    assert!(resized.check_invariants().is_ok());
}

fn shared_window_at_the_edge() -> (Project, Id, Id, Id) {
    let (project, track) = Project::new().add_track("piano");
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(1),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip, Tick::new(0), Tick::new(2))
        .unwrap();
    let (project, other) = project.add_track("link");
    let project = project
        .link_clip(clip, other, Tick::new(i64::MAX - 3), Id::from_raw(77))
        .unwrap();
    project.check_invariants().unwrap();
    (project, track, clip, note)
}

#[test]
fn audit_rea07_edit_of_shared_content_checks_all_origins() {
    let (project, track, clip, note) = shared_window_at_the_edge();
    let edited = project.edit_note(
        track,
        clip,
        note,
        NoteGeometry {
            pitch: 60,
            start: Tick::new(0),
            length: Tick::new(8),
        },
    );
    assert!(edited.is_err());
    assert_eq!(project.tracks()[0].clips.len(), 1);
    assert!(project.check_invariants().is_ok());
}

#[test]
fn audit_rea07_insert_into_shared_content_checks_all_origins() {
    let (project, track, _, _) = shared_window_at_the_edge();
    let inserted = project.insert_note(
        track,
        NoteDraft {
            pitch: 62,
            velocity: 100,
            start: Tick::new(1),
            length: Tick::new(8),
        },
    );
    assert!(inserted.is_err());
    assert_eq!(
        project
            .content(project.tracks()[0].clips[0].content)
            .unwrap()
            .notes
            .len(),
        1
    );
    assert!(project.check_invariants().is_ok());
}
