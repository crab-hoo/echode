use super::*;
use crate::id::Id;
use crate::model::{
    Content, DEFAULT_FADE_FLICKS, Lane, ModelError, Note, NoteDraft, Placement, Track,
};
use crate::time::{Meter, Tempo, TempoMap, Tick};

fn project(contents: Vec<Content>, tracks: Vec<Track>) -> Project {
    Project::unchecked(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        contents,
        tracks,
    )
}

fn track(id: u128, parent: Option<u128>, clips: Vec<Placement>) -> Track {
    Track {
        id: Id::from_raw(id),
        name: format!("track {id}"),
        parent: parent.map(Id::from_raw),
        order: crate::model::sibling_order(id as u32),
        devices: vec![crate::device::test_mixer(id)],
        clips,
    }
}

/// A placement of `bars` 4/4 bars at bar `first`, and its content with one quarter note.
fn clip(id: u128, kind: ClipKind, first: i64, bars: i64) -> (Content, Placement) {
    let content = Content {
        id: Id::from_raw(id + 500),
        audio: None,
        takes: Vec::new(),
        notes: vec![Note {
            id: Id::from_raw(id + 1_000),
            start: Tick::new(0),
            length: Tick::quarters(1).unwrap(),
            pitch: 60,
            velocity: 100,
            release: 0,
            articulation: None,
        }],
        expressions: Vec::new(),
    };
    let placement = Placement {
        id: Id::from_raw(id),
        content: content.id,
        lane: Lane::Timeline,
        start: Tick::quarters(first * 4).unwrap(),
        length: Tick::quarters(bars * 4).unwrap(),
        offset: Tick::new(0),
        kind,
        gain: 1.0,
        fade_in: DEFAULT_FADE_FLICKS,
        fade_out: DEFAULT_FADE_FLICKS,
    };
    (content, placement)
}

fn broken_rule(project: &Project) -> String {
    project.check_invariants().unwrap_err().to_string()
}

#[test]
fn a_new_project_and_an_edited_one_keep_the_rules() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(5).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(Project::new().check_invariants(), Ok(()));
    assert_eq!(project.check_invariants(), Ok(()));
}

#[test]
fn each_broken_rule_is_reported() {
    let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    let reused = project(
        vec![content],
        vec![track(1, None, vec![placement]), track(2, None, Vec::new())],
    );
    assert!(broken_rule(&reused).contains("used twice"));

    let (left_content, left) = clip(4, ClipKind::Fixed, 0, 2);
    let (right_content, right) = clip(5, ClipKind::Fixed, 1, 1);
    let overlapping = project(
        vec![left_content, right_content],
        vec![track(1, None, vec![left, right])],
    );
    assert!(broken_rule(&overlapping).contains("starts before"));

    let (content, placement) = clip(2, ClipKind::Auto, 0, 2);
    let loose_auto = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&loose_auto).contains("auto clip"));

    let orphan = project(Vec::new(), vec![track(1, Some(9), Vec::new())]);
    assert!(broken_rule(&orphan).contains("missing parent"));

    let cycle = project(
        Vec::new(),
        vec![track(1, Some(2), Vec::new()), track(2, Some(1), Vec::new())],
    );
    assert!(broken_rule(&cycle).contains("own ancestor"));

    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.notes.push(Note {
        id: Id::from_raw(8),
        start: Tick::quarters(-1).unwrap(),
        length: Tick::quarters(1).unwrap(),
        release: 0,
        articulation: None,
        pitch: 60,
        velocity: 100,
    });
    let unsorted = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&unsorted).contains("out of start, pitch, id order"));
}

fn assemble(contents: Vec<Content>, tracks: Vec<Track>) -> Result<Project, ModelError> {
    Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        contents,
        tracks,
    )
}

#[test]
fn assembly_checks_auto_bars_from_content_zero_not_timeline_zero() {
    let beat = crate::time::TICKS_PER_QUARTER;
    for origin in [beat / 3, -beat / 3, beat * 9 + 1] {
        let (content, mut placement) = clip(2, ClipKind::Auto, 0, 1);
        placement.start = Tick::new(origin);
        assemble(vec![content.clone()], vec![track(1, None, vec![placement])]).unwrap();

        // This would match the old timeline grid but not this content's bars.
        let (_, mut wrong) = clip(2, ClipKind::Auto, 0, 1);
        wrong.offset = Tick::new(-origin);
        assert!(matches!(
            assemble(vec![content], vec![track(1, None, vec![wrong])]),
            Err(ModelError::Invariant(message)) if message.contains("auto clip")
        ));
    }
}

#[test]
fn assembly_rejects_a_window_a_reversed_pair_an_overflow_and_a_self_take() {
    let (content, placement) = clip(2, ClipKind::Auto, 0, 2);
    let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
    assert!(
        matches!(error, ModelError::Invariant(ref message) if message.contains("auto clip")),
        "{error}"
    );

    let (early_content, early) = clip(4, ClipKind::Fixed, 0, 1);
    let (late_content, late) = clip(5, ClipKind::Fixed, 2, 1);
    let error = assemble(
        vec![early_content, late_content],
        vec![track(1, None, vec![late, early])],
    )
    .unwrap_err();
    assert!(
        matches!(error, ModelError::Invariant(ref message) if message.contains("starts before")),
        "{error}"
    );

    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.notes[0].start = Tick::new(i64::MAX - 1);
    content.notes[0].length = Tick::new(2);
    let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
    assert_eq!(error, ModelError::InvalidNote);

    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.takes = vec![content.id];
    let error = assemble(vec![content], vec![track(1, None, vec![placement])]).unwrap_err();
    assert_eq!(error, ModelError::DuplicateId);
}

#[test]
fn the_shared_check_sees_gain_fade_overflow_takes_and_mixer_ends() {
    let (content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
    placement.gain = -1.0;
    let negative_gain = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&negative_gain).contains("gain"));

    let (content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
    placement.fade_in = -1;
    let negative_fade = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&negative_fade).contains("fade"));

    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.notes[0].start = Tick::new(i64::MAX - 1);
    content.notes[0].length = Tick::new(2);
    let overflow = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&overflow).contains("ends past the time range"));

    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.takes = vec![content.id];
    let self_take = project(vec![content], vec![track(1, None, vec![placement])]);
    assert!(broken_rule(&self_take).contains("lists itself as a take"));

    let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    let mut hosted = track(1, None, vec![placement]);
    hosted.devices[0].parameters[0].max = 5.0;
    let wide = project(vec![content], vec![hosted]);
    assert!(broken_rule(&wide).contains("outside its range"));

    let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    let mut bypassed = track(1, None, vec![placement]);
    bypassed.devices[0].bypassed = true;
    let bypassed = project(vec![content], vec![bypassed]);
    assert!(broken_rule(&bypassed).contains("mixer is bypassed"));

    let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    let mut stateful = track(1, None, vec![placement]);
    stateful.devices[0].state = Some("a".repeat(64));
    let stateful = project(vec![content], vec![stateful]);
    assert!(broken_rule(&stateful).contains("holds plugin state"));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "lists itself as a take")]
fn a_broken_snapshot_panics_where_apply_checks() {
    let (mut content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.takes = vec![content.id];
    let broken = project(vec![content], vec![track(1, None, vec![placement])]);
    broken.debug_assert_invariants();
}

#[test]
fn placement_note_bounds_match_scalar_checks_at_integer_limits() {
    // These cases exercise full content even when notes lie outside the clip window.
    let cases: &[&[(i64, i64)]] = &[
        &[],
        &[(i64::MIN, 1)],
        &[(i64::MAX - 1, 1)],
        &[(-2, 1), (-1, 1), (0, 1), (1, 1)],
        &[(-100, 200), (-1, 1), (0, 1)],
        &[(i64::MIN, 1), (i64::MAX - 1, 1)],
    ];
    for &origin in &[i64::MIN, -100, -1, 0, 1, 100, i64::MAX] {
        for &notes in cases {
            let (mut content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
            let template = content.notes[0].clone();
            content.notes = notes
                .iter()
                .enumerate()
                .map(|(index, &(start, length))| {
                    let mut note = template.clone();
                    note.id = Id::from_raw(100 + index as u128);
                    note.start = Tick::new(start);
                    note.length = Tick::new(length);
                    note
                })
                .collect();
            placement.start = Tick::new(origin.min(0));
            placement.offset = Tick::new(if origin > 0 { -origin } else { 0 });
            placement.length = Tick::new(1);
            let expected = content
                .notes
                .iter()
                .find_map(|note| {
                    // A wider integer oracle is independent of the bounds fast path.
                    let start = i128::from(origin) + i128::from(note.start.get());
                    let last = start + i128::from(note.length.get()) - 1;
                    let range = i128::from(i64::MIN)..=i128::from(i64::MAX);
                    if !range.contains(&start) {
                        Some(format!("note {} starts past the time range", note.id))
                    } else if !range.contains(&last) {
                        Some(format!("note {} ends past the time range", note.id))
                    } else {
                        None
                    }
                })
                .map_or(Ok(()), Err);
            let tracks = vec![track(1, None, vec![placement])];
            let direct = project(vec![content.clone()], tracks.clone())
                .check_invariants()
                .map_err(|error| error.to_string());
            assert_eq!(direct, expected, "origin {origin}, notes {notes:?}");
            let assembled = match assemble(vec![content], tracks) {
                Ok(_) => Ok(()),
                Err(ModelError::Invariant(message)) => Err(message),
                Err(error) => panic!("unexpected assembly error: {error}"),
            };
            assert_eq!(assembled, expected, "origin {origin}, notes {notes:?}");
        }
    }
}

#[test]
fn auto_bars_use_the_earliest_start_and_latest_end_of_shared_content() {
    let bar = Meter::four_four().bar_ticks().unwrap().get();
    let (mut content, mut auto) = clip(2, ClipKind::Auto, 0, 1);
    let template = content.notes[0].clone();
    content.notes[0].start = Tick::new(-bar + 1);
    content.notes[0].length = Tick::new(3 * bar);
    for (id, start) in [(100, 0), (101, bar)] {
        let mut note = template.clone();
        note.id = Id::from_raw(id);
        note.start = Tick::new(start);
        note.length = Tick::new(1);
        content.notes.push(note);
    }
    // The first note has the latest inclusive end, in local bar 2.
    auto.start = Tick::new(37 - bar);
    auto.offset = Tick::new(-bar);
    auto.length = Tick::new(4 * bar);
    let mut fixed = auto.clone();
    fixed.id = Id::from_raw(3);
    fixed.kind = ClipKind::Fixed;
    fixed.start = Tick::new(37 + 3 * bar);
    fixed.offset = Tick::new(10);
    fixed.length = Tick::new(1);
    let mut tracks = vec![track(1, None, vec![auto, fixed])];
    assemble(vec![content.clone()], tracks.clone()).unwrap();
    tracks[0].clips[0].length = Tick::new(3 * bar);
    assert!(matches!(
        assemble(vec![content], tracks),
        Err(ModelError::Invariant(message)) if message.contains("notes cover bars -1..=2")
    ));
}

#[test]
fn empty_content_is_valid_only_for_fixed_placements() {
    let (mut content, mut placement) = clip(2, ClipKind::Fixed, 0, 1);
    content.notes.clear();
    assemble(
        vec![content.clone()],
        vec![track(1, None, vec![placement.clone()])],
    )
    .unwrap();
    placement.kind = ClipKind::Auto;
    assert!(matches!(
        assemble(vec![content], vec![track(1, None, vec![placement])]),
        Err(ModelError::Invariant(message)) if message.contains("has no notes")
    ));
}

#[test]
fn parent_validation_keeps_product_errors_for_self_links_disconnected_cycles_and_orphans() {
    let cases = [
        (vec![track(1, Some(1), Vec::new())], ModelError::Cycle),
        (
            vec![
                track(1, None, Vec::new()),
                track(2, Some(3), Vec::new()),
                track(3, Some(2), Vec::new()),
            ],
            ModelError::Cycle,
        ),
        (
            vec![
                track(1, Some(2), Vec::new()),
                track(2, Some(3), Vec::new()),
                track(3, Some(2), Vec::new()),
            ],
            ModelError::Cycle,
        ),
        (
            vec![
                track(1, Some(2), Vec::new()),
                track(2, Some(3), Vec::new()),
                track(3, Some(99), Vec::new()),
            ],
            ModelError::MissingTrack,
        ),
    ];
    for (tracks, expected) in cases {
        assert_eq!(assemble(Vec::new(), tracks.clone()).unwrap_err(), expected);
        let message = broken_rule(&project(Vec::new(), tracks));
        match expected {
            ModelError::Cycle => assert!(message.contains("is its own ancestor"), "{message}"),
            ModelError::MissingTrack => assert!(message.contains("missing parent"), "{message}"),
            _ => unreachable!(),
        }
    }
}

#[test]
fn separate_roots_and_branches_can_share_resolved_ancestors() {
    let tracks = vec![
        track(1, None, Vec::new()),
        track(2, Some(1), Vec::new()),
        track(3, Some(2), Vec::new()),
        track(4, Some(1), Vec::new()),
        track(5, None, Vec::new()),
    ];
    assemble(Vec::new(), tracks.clone()).unwrap();
    project(Vec::new(), tracks).check_invariants().unwrap();
}
