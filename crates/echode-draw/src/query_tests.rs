use super::*;
use echode_model::{
    ClipKind, Content, DEFAULT_FADE_FLICKS, Lane, Meter, Note, NoteDraft, Placement, Project,
    TICKS_PER_QUARTER, Tempo, TempoMap, Tick, Track, initial_order, midi7_to_velocity, test_mixer,
};

fn view_of(origin: i64, width: i64, tracks: f64) -> ArrangeView {
    ArrangeView {
        origin_tick: Tick::new(origin),
        width_ticks: width,
        origin_track: 0.0,
        height_tracks: tracks,
        pixels_per_tick: 1.0,
        pixels_per_track: 20.0,
    }
}

fn piano() -> (Project, Id) {
    Project::new().add_track("piano")
}

fn quarter(start_quarters: i64, length_quarters: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(start_quarters * TICKS_PER_QUARTER),
        length: Tick::new(length_quarters * TICKS_PER_QUARTER),
    }
}

#[test]
fn a_note_that_starts_before_the_view_is_visible_until_it_ends() {
    let (project, track) = piano();
    let (project, long) = project.insert_note(track, quarter(0, 20)).unwrap();
    let (project, early) = project.insert_note(track, quarter(0, 1)).unwrap();
    let index = ArrangeIndex::build(&project);
    let mut out = [Element {
        id: track_only(track),
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }; 8];
    let (written, dropped) = index.visible(
        &project,
        view_of(10 * TICKS_PER_QUARTER, TICKS_PER_QUARTER, 1.0),
        &mut out,
    );
    assert_eq!(dropped, 0);
    let notes: Vec<_> = out[..written as usize]
        .iter()
        .filter(|element| element.id.role == Role::Note)
        .map(|element| element.id.note)
        .collect();
    assert_eq!(notes, vec![Some(long)]);
    assert!(!notes.contains(&Some(early)));
}

#[test]
fn a_note_outside_the_placement_window_is_hidden() {
    let (project, track) = piano();
    let (project, outside) = project.insert_note(track, quarter(0, 1)).unwrap();
    let (project, inside) = project.insert_note(track, quarter(1, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(
            track,
            clip,
            Tick::new(TICKS_PER_QUARTER),
            Tick::new(TICKS_PER_QUARTER),
        )
        .unwrap();
    let index = ArrangeIndex::build(&project);
    let mut out = [Element {
        id: track_only(track),
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }; 8];
    let (written, _) = index.visible(&project, view_of(0, 20 * TICKS_PER_QUARTER, 1.0), &mut out);
    let notes: Vec<_> = out[..written as usize]
        .iter()
        .filter(|element| element.id.role == Role::Note)
        .map(|element| element.id.note)
        .collect();
    assert_eq!(notes, vec![Some(inside)]);
    assert!(!notes.contains(&Some(outside)));
}

#[test]
fn one_note_in_two_placements_is_two_elements() {
    let (project, track) = piano();
    let (project, note) = project.insert_note(track, quarter(0, 1)).unwrap();
    let (project, other) = project.add_track("copy");
    let source = project.tracks()[0].clips[0].id;
    let link = Id::from_raw(42);
    let project = project
        .link_clip(source, other, Tick::new(0), link)
        .unwrap();
    let index = ArrangeIndex::build(&project);
    let mut out = [Element {
        id: track_only(track),
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }; 16];
    let (written, dropped) =
        index.visible(&project, view_of(0, 4 * TICKS_PER_QUARTER, 2.0), &mut out);
    assert_eq!(dropped, 0);
    let notes: Vec<_> = out[..written as usize]
        .iter()
        .filter(|element| element.id.role == Role::Note)
        .map(|element| (element.id.placement, element.id.note))
        .collect();
    assert_eq!(
        notes,
        vec![(Some(source), Some(note)), (Some(link), Some(note))]
    );
}

#[test]
fn a_short_buffer_counts_the_overflow_and_a_hit_prefers_the_note() {
    let (project, track) = piano();
    let (project, first) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                ..quarter(0, 1)
            },
        )
        .unwrap();
    let (project, second) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                ..quarter(0, 1)
            },
        )
        .unwrap();
    let index = ArrangeIndex::build(&project);
    let mut one = [Element {
        id: track_only(track),
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }; 1];
    let (written, dropped) = index.visible(&project, view_of(0, TICKS_PER_QUARTER, 1.0), &mut one);
    assert_eq!(written, 1);
    assert!(dropped >= 1);
    let view = view_of(0, TICKS_PER_QUARTER, 1.0);
    let hit = index
        .hit(&project, view, (TICKS_PER_QUARTER / 2) as f32, 10.0)
        .unwrap();
    assert_eq!(hit.role, Role::Note);
    assert_eq!(hit.note, Some(second));
    assert_ne!(hit.note, Some(first));
    let lane = index
        .hit(&project, view, (TICKS_PER_QUARTER / 2) as f32, 1.0)
        .unwrap();
    assert_eq!(lane.role, Role::Clip);
}

#[test]
fn logical_order_leaves_the_view_and_focus_survives_a_scroll() {
    let (project, track) = piano();
    let (project, early) = project.insert_note(track, quarter(0, 1)).unwrap();
    let (project, late) = project.insert_note(track, quarter(1, 1)).unwrap();
    let index = ArrangeIndex::build(&project);
    let view = view_of(0, TICKS_PER_QUARTER, 1.0);
    let mut out = [Element {
        id: track_only(track),
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }; 8];
    let (written, _) = index.visible(&project, view, &mut out);
    let visible_notes: Vec<_> = out[..written as usize]
        .iter()
        .filter(|element| element.id.role == Role::Note)
        .map(|element| element.id.note)
        .collect();
    assert_eq!(visible_notes, vec![Some(early)]);
    let early_id = out
        .iter()
        .find(|element| element.id.note == Some(early))
        .unwrap()
        .id;
    let next = index.logical_next(&project, Some(early_id)).unwrap();
    assert_eq!(next.note, Some(late));
    assert!(index.retain(&project, next).unwrap().note == Some(late));
    let revealed = index.reveal(&project, view, next);
    let (written, _) = index.visible(&project, revealed, &mut out);
    let shown: Vec<_> = out[..written as usize]
        .iter()
        .filter(|element| element.id.role == Role::Note)
        .map(|element| element.id.note)
        .collect();
    assert_eq!(shown, vec![Some(late)]);
}

#[test]
fn a_deleted_note_keeps_focus_on_its_placement() {
    let (project, track) = piano();
    let (project, keep) = project.insert_note(track, quarter(0, 1)).unwrap();
    let (project, gone) = project.insert_note(track, quarter(1, 1)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let focus = ElementId {
        role: Role::Note,
        track,
        placement: Some(clip),
        note: Some(gone),
    };
    let index = ArrangeIndex::build(&project);
    let project = project.delete_note(track, gone).unwrap();
    let index = index.update(&project);
    let kept = index.retain(&project, focus).unwrap();
    assert_eq!(kept.role, Role::Clip);
    assert_eq!(kept.placement, Some(clip));
    assert!(
        index
            .logical_next(&project, Some(track_only(track)))
            .is_some()
    );
    let _ = keep;
}

#[test]
fn an_unrelated_edit_reuses_the_content_and_placement_trees() {
    let (project, track) = piano();
    let (project, _) = project.insert_note(track, quarter(0, 4)).unwrap();
    let index = ArrangeIndex::build(&project);
    let gain = project.tracks()[0].devices[0].parameters[0].id;
    let quieter = project.set_parameter(gain, 0.5).unwrap();
    let updated = index.update(&quieter);
    assert!(index.shares_content(&updated));
    assert!(index.shares_placement(&updated, track));
    let clip = project.tracks()[0].clips[0].id;
    let resized = project
        .resize_clip(track, clip, Tick::new(0), Tick::new(2 * TICKS_PER_QUARTER))
        .unwrap();
    let moved = index.update(&resized);
    assert!(index.shares_content(&moved));
    assert!(!index.shares_placement(&moved, track));
}

#[test]
fn sparse_long_notes_do_not_walk_the_whole_content() {
    // This population is the UI-1 check for a sparse overlap, not a product limit.
    const NOTES: usize = 1 << 20;
    const LONG: usize = 1024;
    let window = (NOTES as i64 - 1) * 2 + 1;
    let mut notes = Vec::with_capacity(NOTES);
    for index in 0..NOTES {
        let start = index as i64 * 2;
        let long = index % (NOTES / LONG) == 0;
        let length = if long { window + 1 - start } else { 1 };
        notes.push(Note {
            id: Id::from_raw(1_000_000 + index as u128),
            start: Tick::new(start),
            length: Tick::new(length),
            pitch: 60,
            velocity: midi7_to_velocity(100),
            release: 0,
            articulation: None,
        });
    }
    let content_id = Id::from_raw(11);
    let track_id = Id::from_raw(12);
    let clip_id = Id::from_raw(13);
    let content = Content {
        id: content_id,
        audio: None,
        takes: Vec::new(),
        notes,
        expressions: Vec::new(),
    };
    let clip = Placement {
        id: clip_id,
        content: content_id,
        lane: Lane::Timeline,
        start: Tick::new(0),
        length: Tick::new(window + 1),
        offset: Tick::new(0),
        kind: ClipKind::Fixed,
        gain: 1.0,
        fade_in: DEFAULT_FADE_FLICKS,
        fade_out: DEFAULT_FADE_FLICKS,
    };
    let track = Track {
        id: track_id,
        name: "sparse".into(),
        parent: None,
        order: initial_order(),
        clips: vec![clip],
        devices: vec![test_mixer(12)],
    };
    let project = Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        vec![content],
        vec![track],
    )
    .unwrap();
    let index = ArrangeIndex::build(&project);
    let content = project.content(content_id).unwrap();
    let (hits, visits) = index.note_visits(content, window, window + 1);
    assert_eq!(hits, LONG as u32);
    assert!(visits < (NOTES as u64) / 8);
}
