use echode_draw::{ArrangeIndex, PianoView};
use echode_model::{Id, NoteDraft, NoteGeometry, Project, Tick};

fn note(start: i64, length: i64, pitch: u8) -> NoteDraft {
    NoteDraft {
        start: Tick::quarters(start).unwrap(),
        length: Tick::quarters(length).unwrap(),
        pitch,
        velocity: 100,
    }
}
fn view(start: i64, top: f64) -> PianoView {
    PianoView {
        origin_tick: Tick::quarters(start).unwrap(),
        width_ticks: 705_600 * 2,
        top_pitch: top,
        height_pitches: 2.,
        pixels_per_tick: 1. / 705_600.,
        pixels_per_pitch: 20.,
    }
}

#[test]
fn long_notes_are_clipped_to_time_and_pitch_rows_are_half_open() {
    let (project, track) = Project::new().add_track("keys");
    let (project, long) = project.insert_note(track, note(0, 20, 60)).unwrap();
    let (project, _) = project.insert_note(track, note(10, 1, 58)).unwrap();
    let (project, _) = project.insert_note(track, note(10, 1, 62)).unwrap();
    let index = ArrangeIndex::build(&project);
    let mut shown = Vec::new();
    index.visit_piano(&project, track, view(10, 61.), |item| shown.push(item));
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id.note, Some(long));
    assert_eq!(
        (shown[0].x, shown[0].y, shown[0].w, shown[0].h),
        (0., 20., 2., 20.)
    );
    assert_eq!(shown[0].start, Tick::new(0));
    shown.clear();
    index.visit_piano(&project, track, view(10, 59.5), |item| shown.push(item));
    assert_eq!(shown.len(), 2); // Part of pitch 60 and part of pitch 58.
    shown.clear();
    let mut invalid = view(10, f64::NAN);
    index.visit_piano(&project, track, invalid, |item| shown.push(item));
    invalid = view(10, 60.);
    invalid.origin_tick = Tick::new(i64::MAX);
    index.visit_piano(&project, track, invalid, |item| shown.push(item));
    assert!(shown.is_empty());
}

#[test]
fn linked_notes_keep_distinct_ids_and_fixed_windows_clip_the_sound() {
    let (project, track) = Project::new().add_track("keys");
    let (project, other) = project.add_track("linked");
    let (project, id) = project.insert_note(track, note(0, 8, 60)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let linked = Id::generate();
    let project = project
        .link_clip(clip, other, Tick::quarters(32).unwrap(), linked)
        .unwrap()
        .resize_clip(
            other,
            linked,
            Tick::quarters(34).unwrap(),
            Tick::quarters(1).unwrap(),
        )
        .unwrap();
    let index = ArrangeIndex::build(&project);
    let mut shown = Vec::new();
    index.visit_piano(&project, other, view(33, 60.), |item| shown.push(item));
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id.note, Some(id));
    assert_eq!(shown[0].id.placement, Some(linked));
    assert_eq!((shown[0].x, shown[0].w), (1., 1.));
    assert_eq!(shown[0].start, Tick::quarters(32).unwrap());
}

#[test]
fn navigation_leaves_the_view_and_resolves_a_retired_placement_after_merge() {
    let (project, track) = Project::new().add_track("keys");
    let (project, first) = project.insert_note(track, note(0, 1, 60)).unwrap();
    let (project, last) = project.insert_note(track, note(100, 1, 90)).unwrap();
    let index = ArrangeIndex::build(&project);
    let a = index.piano_neighbor(&project, track, None, false).unwrap();
    let b = index
        .piano_neighbor(&project, track, Some(a.id), false)
        .unwrap();
    assert_eq!(a.id.note, Some(first));
    assert_eq!(b.id.note, Some(last));
    assert!(
        index
            .piano_neighbor(&project, track, Some(b.id), false)
            .is_none()
    );
    assert_eq!(
        index
            .piano_neighbor(&project, track, Some(b.id), true)
            .unwrap()
            .id,
        a.id
    );
    let edited = project
        .edit_note(
            track,
            b.id.placement.unwrap(),
            last,
            NoteGeometry {
                pitch: 61,
                start: Tick::quarters(2).unwrap(),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap();
    let updated = index.update(&edited);
    let resolved = updated
        .piano_note(&edited, track, b.id.placement, last)
        .unwrap();
    assert_eq!(resolved.id.placement, a.id.placement);
    assert_eq!(resolved.start, Tick::quarters(2).unwrap());
}
