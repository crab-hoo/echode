use super::*;
use echode_realtime::{Event, NoteSpan, Plan, Span};

fn track(geometry: &[(i64, i64)]) -> TrackPlan {
    let mut notes: Vec<_> = geometry
        .iter()
        .enumerate()
        .map(|(index, &(on, off))| NoteSpan {
            on,
            off,
            note: index as u128 + 1,
            note_id: index as u32,
            pitch: 60,
            velocity: u16::MAX,
        })
        .collect();
    notes.sort_by_key(|note| note.on);
    let mut by_off: Vec<_> = (0..notes.len() as u32).collect();
    by_off.sort_by_key(|index| notes[*index as usize].off);
    TrackPlan {
        track: 1,
        track_key: 1,
        notes,
        by_off,
        gain: 1.0,
        pan: 0.0,
        mute: 0.0,
        solo: 0.0,
    }
}

#[test]
fn density_and_overlap_not_score_length_set_the_bound() {
    let short = track(&[(0, 2), (10, 12)]);
    let long = track(&(0..2048).map(|i| (i * 10, i * 10 + 2)).collect::<Vec<_>>());
    for frames in [1, 2, 3, 8] {
        for bounds in [None, Some((0, 1)), Some((0, 3)), Some((0, 20))] {
            let a = track_event_demand(&short, bounds, frames).unwrap();
            let b = track_event_demand(&long, bounds, frames).unwrap();
            assert_eq!(a.events, b.events);
            assert_eq!(b.chase, 1);
        }
    }
    let chord = track(&[(0, 5); 128]); // Fixture polyphony only, not a product limit.
    let demand = track_event_demand(&chord, None, 1).unwrap();
    assert_eq!(demand.chase, 128);
    assert_eq!(demand.events, 256);
    let adjacent = track(&[(0, 2), (2, 4), (4, 6)]);
    assert_eq!(track_event_demand(&adjacent, None, 1).unwrap().chase, 1);
}

#[test]
fn cursor_events_fit_for_all_small_loop_windows_and_landings() {
    // Exhaustive geometry is confined to this fixture's sample range. Include
    // exact ends, pre-loop chased notes, chords and several wraps in one block.
    let geometries = [
        vec![],
        vec![(0, 1)],
        vec![(-4, 10)],
        vec![(-4, 0), (0, 2), (2, 4), (4, 8)],
        vec![(-3, 1), (-3, 2), (0, 3), (0, 4), (1, 4), (2, 7)],
    ];
    for geometry in geometries {
        let plan = Plan {
            tracks: vec![track(&geometry)],
        };
        for bounds in [
            None,
            Some((0, 1)),
            Some((0, 3)),
            Some((-2, 4)),
            Some((2, 8)),
        ] {
            for frames in 1..=12 {
                let demand = track_event_demand(&plan.tracks[0], bounds, frames).unwrap();
                for landing in -5..=10 {
                    let mut cursor = plan.cursor();
                    cursor.catch_up(&plan, landing);
                    let mut events =
                        vec![Event::note_off(0, 0, 0, 0); demand.events + demand.chase];
                    let chase = cursor.chase_events(&plan, &mut events).unwrap();
                    assert!(chase <= demand.chase);
                    let mut order = vec![0; events.len()];
                    let mut scratch = events.clone();
                    let mut spans = vec![
                        Span {
                            start: 0,
                            frames: 0,
                            block_offset: 0,
                            release_at: None
                        };
                        frames
                    ];
                    let (start, end) = bounds
                        .filter(|(_, end)| landing < *end)
                        .map_or((None, None), |(start, end)| (Some(start), Some(end)));
                    let (count, _) =
                        super::super::split_loop(landing, frames, start, end, &mut spans).unwrap();
                    cursor
                        .play(
                            &plan,
                            &spans[..count],
                            &mut events[chase..],
                            &mut order,
                            &mut scratch,
                        )
                        .unwrap_or_else(|error| {
                            panic!("{geometry:?}, {bounds:?}, {frames}, {landing}: {error:?}")
                        });
                }
            }
        }
    }
}

#[test]
fn empty_and_extreme_timelines_do_not_overflow() {
    for geometry in [
        vec![],
        vec![(i64::MIN, i64::MIN + 1), (i64::MAX - 1, i64::MAX)],
    ] {
        let track = track(&geometry);
        for bounds in [None, Some((i64::MIN, i64::MAX)), Some((0, 1))] {
            assert!(track_event_demand(&track, bounds, 64).is_ok());
        }
    }
    let track = track(&[(0, 2)]);
    assert!(track_event_demand(&track, None, 0).is_err());
    assert!(track_event_demand(&track, Some((2, 2)), 64).is_err());
    assert!(track_event_demand(&track, Some((2, 1)), 64).is_err());
}
