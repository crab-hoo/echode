use super::*;
use crate::{Lane, Project, test_mixer};

fn merging_project(edge_link: bool) -> Project {
    let template = Project::new();
    let bar = template.meter().bar_ticks().unwrap().get();
    let contents = [(10, 11), (20, 21)]
        .into_iter()
        .map(|(id, note)| Content {
            id: Id::from_raw(id),
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(note),
                start: Tick::new(0),
                length: Tick::new(1),
                pitch: 60,
                velocity: 12345,
                release: 23456,
                articulation: None,
            }]
            .into(),
            expressions: Vec::new(),
        })
        .collect();
    let mut auto = track(1000, Vec::new());
    for (id, content, start) in [(12, 10, 0), (22, 20, 2 * bar)] {
        let mut clip = placement(Id::from_raw(content), 0);
        clip.id = Id::from_raw(id);
        clip.start = Tick::new(start);
        clip.length = Tick::new(bar);
        clip.kind = ClipKind::Auto;
        auto.clips.push(clip);
    }
    let mut link = placement(Id::from_raw(20), 0);
    link.id = Id::from_raw(32);
    link.start = Tick::new(-bar);
    link.length = Tick::new(2 * bar);
    link.offset = Tick::new(-bar);
    let mut linked = track(2000, vec![link]);
    linked.order = crate::model::sibling_order(1);
    let mut tracks = vec![auto, linked];
    if edge_link {
        let mut link = placement(Id::from_raw(10), i64::MAX - 1);
        link.id = Id::from_raw(42);
        link.start = Tick::new(i64::MAX - 1);
        link.offset = Tick::new(0);
        let mut edge = track(3000, vec![link]);
        edge.order = crate::model::sibling_order(2);
        tracks.push(edge);
    }
    Project::try_assemble(template.meter(), template.tempo().clone(), contents, tracks).unwrap()
}

fn merge_edit(
    project: &Project,
    through_link: bool,
    operation: &str,
) -> Result<Project, ModelError> {
    let bar = project.meter().bar_ticks().unwrap().get();
    let (track, clip, start) = if through_link {
        (Id::from_raw(2000), Id::from_raw(32), -bar)
    } else {
        (Id::from_raw(1000), Id::from_raw(22), bar)
    };
    match operation {
        "edit" => project.edit_note(
            track,
            clip,
            Id::from_raw(21),
            NoteGeometry {
                pitch: 61,
                start: Tick::new(start),
                length: Tick::new(1),
            },
        ),
        "duplicate" => project.duplicate_note(
            track,
            clip,
            Id::from_raw(21),
            Id::from_raw(40000),
            61,
            Tick::new(start),
        ),
        "insert" | "insert_existing" => project.insert_note_with_id(
            track,
            Id::from_raw(40000),
            Id::from_raw(40001),
            Id::from_raw(40002),
            NoteDraft {
                pitch: 61,
                velocity: 100,
                start: Tick::new(if operation == "insert_existing" && !through_link {
                    0
                } else {
                    start
                }),
                length: Tick::new(if operation == "insert_existing" {
                    2 * bar + 1
                } else {
                    1
                }),
            },
        ),
        _ => unreachable!(),
    }
}

#[test]
fn merged_note_edits_validate_the_surviving_content_on_either_track() {
    for through_link in [false, true] {
        for operation in ["edit", "duplicate", "insert", "insert_existing"] {
            let project = merging_project(false);
            let edited = merge_edit(&project, through_link, operation).unwrap();
            edited.check_invariants().unwrap();
            project.check_invariants().unwrap();
            assert!(edited.content(Id::from_raw(20)).is_none());
            assert_eq!(edited.tracks()[0].clips.len(), 1);
            let primary = &edited.tracks()[0].clips[0];
            assert_eq!(primary.id, Id::from_raw(12));
            assert_eq!(primary.content, Id::from_raw(10));
            let linked = &edited.tracks()[1].clips[0];
            assert_eq!(linked.content, primary.content);
            assert_eq!(linked.start, project.tracks()[1].clips[0].start);
            assert_eq!(linked.length, project.tracks()[1].clips[0].length);
            let content = edited.content(primary.content).unwrap();
            let note = content.notes.iter().find(|note| note.pitch == 61).unwrap();
            let bar = project.meter().bar_ticks().unwrap().get();
            let local = if operation == "insert_existing" && !through_link {
                0
            } else {
                bar
            };
            assert_eq!(note.start.get(), local);
            assert_eq!(primary.sounding_start(note).unwrap().get(), local);
            assert_eq!(linked.sounding_start(note).unwrap().get(), local - 2 * bar);
            if operation == "duplicate" {
                assert_eq!(note.velocity, 12345);
                assert_eq!(note.release, 23456);
            }
            assert_eq!(project.contents().len(), 2);
            assert_eq!(project.content(Id::from_raw(20)).unwrap().notes.len(), 1);
            assert_eq!(
                project.content(Id::from_raw(20)).unwrap().notes[0].pitch,
                60
            );
        }
    }
}

#[test]
fn merged_note_edits_reject_overflow_in_links_of_the_surviving_content() {
    for through_link in [false, true] {
        for operation in ["edit", "duplicate", "insert", "insert_existing"] {
            let project = merging_project(true);
            assert_eq!(
                merge_edit(&project, through_link, operation).unwrap_err(),
                ModelError::InvalidNote,
                "{operation}, through_link={through_link}",
            );
            project.check_invariants().unwrap();
            assert_eq!(project.contents().len(), 2);
            assert_eq!(project.tracks()[0].clips.len(), 2);
            assert_eq!(
                project.content(Id::from_raw(20)).unwrap().notes[0]
                    .start
                    .get(),
                0
            );
        }
    }
}

fn placement(content: Id, origin: i64) -> Placement {
    Placement {
        id: Id::from_raw(2),
        content,
        lane: Lane::Timeline,
        start: Tick::new(origin.min(0)),
        length: Tick::new(1),
        offset: Tick::new(if origin > 0 { -origin } else { 0 }),
        kind: ClipKind::Fixed,
        gain: 1.0,
        fade_in: 0,
        fade_out: 0,
    }
}

fn track(id: u128, clips: Vec<Placement>) -> Track {
    Track {
        id: Id::from_raw(id),
        name: "bounds".into(),
        parent: None,
        order: crate::initial_order(),
        clips,
        devices: vec![test_mixer(id)],
    }
}

#[test]
fn shared_note_bounds_match_scalar_checks_at_integer_limits() {
    // These cases bound this regression, not product notes or time positions.
    let cases: &[&[(i64, i64)]] = &[
        &[],
        &[(i64::MIN, 1)],
        &[(i64::MAX - 1, 1)],
        &[(i64::MAX, 1)],
        &[(i64::MAX - 1, 2)],
        &[(-2, 1), (-1, 1), (0, 1), (1, 1)],
        &[(-100, 200), (-1, 1), (0, 1)],
        &[(i64::MIN, 1), (i64::MAX - 1, 1)],
    ];
    for &origin in &[i64::MIN, -100, -1, 0, 1, 100, i64::MAX] {
        for &notes in cases {
            let content = Arc::new(Content {
                id: Id::from_raw(1),
                audio: None,
                takes: Vec::new(),
                notes: notes
                    .iter()
                    .enumerate()
                    .map(|(index, &(start, length))| Note {
                        id: Id::from_raw(100 + index as u128),
                        start: Tick::new(start),
                        length: Tick::new(length),
                        pitch: 60,
                        velocity: 0,
                        release: 0,
                        articulation: None,
                    })
                    .collect(),
                expressions: Vec::new(),
            });
            let expected = notes.iter().all(|&(start, length)| {
                // A wider scalar oracle does not depend on the extrema fast path.
                let local_end = i128::from(start) + i128::from(length);
                let absolute = i128::from(origin) + i128::from(start);
                let last = absolute + i128::from(length) - 1;
                let range = i128::from(i64::MIN)..=i128::from(i64::MAX);
                range.contains(&local_end) && range.contains(&absolute) && range.contains(&last)
            });
            for on_current_track in [true, false] {
                let clip = placement(content.id, origin);
                let mut current = track(3, Vec::new());
                let mut other = track(4, Vec::new());
                if on_current_track {
                    current.clips.push(clip);
                } else {
                    other.clips.push(clip);
                }
                let tracks = [Arc::new(current.clone()), Arc::new(other)];
                let result = check_shared_notes(
                    std::slice::from_ref(&content),
                    &tracks,
                    &current,
                    content.id,
                );
                assert_eq!(
                    result.is_ok(),
                    expected,
                    "origin {origin}, notes {notes:?}, on current track {on_current_track}"
                );
                if !expected {
                    assert_eq!(result, Err(ModelError::InvalidNote));
                }
            }
        }
    }
}

#[test]
fn shared_note_edits_keep_inclusive_time_limits_and_reject_overflow_atomically() {
    for on_current_track in [true, false] {
        for upper_edge in [true, false] {
            let (project, source) = Project::new().add_track("source");
            let (project, note) = project
                .insert_note(
                    source,
                    NoteDraft {
                        pitch: 60,
                        velocity: 100,
                        start: Tick::new(0),
                        length: Tick::new(5),
                    },
                )
                .unwrap();
            let (project, _) = project
                .insert_note(
                    source,
                    NoteDraft {
                        pitch: 62,
                        velocity: 100,
                        start: Tick::new(4),
                        length: Tick::new(1),
                    },
                )
                .unwrap();
            let clip = project.tracks()[0].clips[0].id;
            let project = project
                .resize_clip(source, clip, Tick::new(0), Tick::new(10))
                .unwrap();
            let (project, target) = if on_current_track {
                (project, source)
            } else {
                project.add_track("linked")
            };
            let origin = if upper_edge {
                i64::MAX - 10
            } else {
                i64::MIN + 10
            };
            let project = project
                .link_clip(clip, target, Tick::new(origin), Id::from_raw(77))
                .unwrap();
            let before = project.clone();
            for overflow in [false, true] {
                let extra = i64::from(overflow);
                let geometry = NoteGeometry {
                    pitch: 61,
                    start: Tick::new(if upper_edge { 0 } else { -10 - extra }),
                    length: Tick::new(if upper_edge { 11 + extra } else { 5 }),
                };
                let result = project.edit_note(source, clip, note, geometry);
                if overflow {
                    assert_eq!(result, Err(ModelError::InvalidNote));
                } else {
                    let edited = result.unwrap();
                    edited.check_invariants().unwrap();
                    let content = edited
                        .content(project.tracks()[0].clips[0].content)
                        .unwrap();
                    let changed = content.notes.iter().find(|item| item.id == note).unwrap();
                    assert_eq!(changed.start, geometry.start);
                    assert_eq!(changed.length, geometry.length);
                    assert_eq!(changed.pitch, 61);
                }
                assert_eq!(project, before);
                assert!(Arc::ptr_eq(&project.contents()[0], &before.contents()[0]));
                project.check_invariants().unwrap();
            }
        }
    }
}

#[test]
fn fixed_bar_index_matches_scalar_overlap_queries() {
    // The cases bound this helper regression, not product time or clip counts.
    // Include unsorted, nested and invalid intermediate windows to preserve the
    // old predicate independently of snapshot ordering and final validation.
    let windows = [
        (5, 20, ClipKind::Fixed),
        (-20, 9, ClipKind::Fixed),
        (0, 1, ClipKind::Auto),
        (i64::MIN, 1, ClipKind::Fixed),
        (i64::MAX, 1, ClipKind::Fixed),
        (i64::MAX, 2, ClipKind::Fixed),
        (3, 0, ClipKind::Fixed),
        (8, -1, ClipKind::Fixed),
        (-5, 1, ClipKind::Fixed),
        (7, 1, ClipKind::Fixed),
    ];
    let queries = [
        (-30, -21),
        (-20, -20),
        (-3, -1),
        (0, 0),
        (1, 1),
        (2, 4),
        (5, 7),
        (-12, -12),
        (-11, -11),
        (20, 20),
        (24, 24),
        (25, 25),
        (26, 30),
        (i64::MIN, i64::MIN),
        (i64::MAX, i64::MAX),
        (i64::MIN, i64::MAX),
    ];
    let mut clips: Vec<_> = windows
        .into_iter()
        .map(|(start, length, kind)| {
            let mut clip = placement(Id::from_raw(1), 0);
            clip.start = Tick::new(start);
            clip.length = Tick::new(length);
            clip.kind = kind;
            clip
        })
        .collect();
    for bar in [1, 4, 100, Project::new().meter().bar_ticks().unwrap().get()] {
        for _ in 0..clips.len() {
            clips.rotate_left(1);
            let fixed = FixedBars::new(&clips, bar);
            for left in queries {
                for right in queries {
                    let low = left.0.min(right.0);
                    let high = left.1.max(right.1);
                    let expected = clips.iter().any(|clip| {
                        let start = i128::from(clip.start.get());
                        let last = start + i128::from(clip.length.get()) - 1;
                        clip.kind == ClipKind::Fixed
                            && clip.length.get() > 0
                            && last <= i128::from(i64::MAX)
                            && start.div_euclid(i128::from(bar)) <= i128::from(high)
                            && i128::from(low) <= last.div_euclid(i128::from(bar))
                    });
                    assert_eq!(
                        fixed.blocks(left, right),
                        expected,
                        "bar {bar}, left {left:?}, right {right:?}"
                    );
                }
            }
        }
    }
    for clips in [
        Vec::new(),
        clips
            .into_iter()
            .filter(|clip| clip.kind == ClipKind::Auto)
            .collect(),
    ] {
        assert!(!FixedBars::new(&clips, 1).blocks((i64::MIN, 0), (0, i64::MAX)));
    }
}
