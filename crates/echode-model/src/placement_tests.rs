use super::*;
use crate::{Lane, Project, test_mixer};

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
