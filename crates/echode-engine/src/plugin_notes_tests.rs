use super::*;
use echode_realtime::NoteSpan;

fn score(geometry: &[(i64, i64, u32, u8)]) -> Score {
    let track = TrackPlan {
        track: 1,
        track_key: 1,
        notes: geometry
            .iter()
            .map(|&(on, off, id, pitch)| NoteSpan {
                on,
                off,
                note: u128::from(id),
                note_id: id,
                pitch,
                velocity: 65535,
            })
            .collect(),
        by_off: (0..geometry.len() as u32).collect(),
        gain: 1.0,
        pan: 0.0,
        mute: 0.0,
        solo: 0.0,
    };
    Score::new(&track, Arc::new(NoteIndex::new(&track)))
}

fn notes(score: Score, block: usize) -> Notes {
    // Only this fixture's density, releases and chase, not a product event limit.
    Notes::new(score, block, 128).unwrap()
}

#[test]
fn held_notes_wait_for_the_source_and_stop_without_future_ons_under_rtsan() {
    let score = score(&[(10, 100, 1, 60), (101, 200, 2, 61)]);
    let mut notes = notes(score, 32);
    let mut head = Playhead::new(50, (None, None));
    {
        let _guard = crate::ForbidAlloc::enter();
        notes.prepare(head, 37, 32, true).unwrap();
    }
    assert!(notes.events().unwrap().is_empty());
    head.advance(32, 37).unwrap();
    {
        let _guard = crate::ForbidAlloc::enter();
        notes.prepare(head, 37, 32, true).unwrap();
    }
    assert_eq!(notes.events().unwrap(), &[Event::note_on(5, 1, 1, 60, 1.0)]);
    {
        let _guard = crate::ForbidAlloc::enter();
        notes.reset();
        notes.stop().unwrap();
    }
    assert_eq!(notes.events().unwrap(), &[Event::note_off(0, 1, 1, 60)]);
    notes.stop().unwrap();
    assert!(notes.events().unwrap().is_empty());
    assert!(!notes.active());
}

#[test]
fn exact_loop_landings_release_before_restarting_and_never_chase_before_the_loop_under_rtsan() {
    for block in [1, 2, 8, 13, 32] {
        let mut notes = notes(score(&[(-10, 100, 1, 59), (0, 100, 2, 60)]), block);
        let mut head = Playhead::new(-3, (Some(0), Some(8)));
        let mut actual = Vec::new();
        let mut elapsed = 0;
        while elapsed < 100 {
            let frames = block.min(100 - elapsed);
            {
                let _guard = crate::ForbidAlloc::enter();
                notes.prepare(head, 13, frames, false).unwrap();
            }
            actual.extend(
                notes
                    .events()
                    .unwrap()
                    .iter()
                    .chain(notes.chased().unwrap())
                    .map(|event| {
                        (
                            elapsed + event.time() as usize,
                            event.is_note_on(),
                            event.note_id().unwrap(),
                        )
                    }),
            );
            head.advance(frames, 13).unwrap();
            elapsed += frames;
        }
        actual.sort_unstable();
        let mut expected = vec![(13, true, 1), (16, true, 2), (24, false, 1)];
        for at in (24..100).step_by(8) {
            expected.push((at, false, 2));
            expected.push((at, true, 2));
        }
        expected.sort_unstable();
        assert_eq!(actual, expected, "block={block}");
    }
}

#[test]
fn plan_handover_keeps_held_keys_releases_deletes_and_handles_latency_growth_under_rtsan() {
    let score = score(&[(0, 100, 1, 60)]);
    let mut old = notes(score.clone(), 8);
    let mut head = Playhead::new(0, (None, None));
    old.prepare(head, 0, 8, true).unwrap();
    head.advance(8, 0).unwrap();
    let mut same = notes(score, 8);
    {
        let _guard = crate::ForbidAlloc::enter();
        same.carry(&mut old, head, 0).unwrap();
        same.prepare(head, 0, 8, false).unwrap();
    }
    assert!(same.events().unwrap().is_empty());
    assert_eq!(same.chased().unwrap(), &[Event::note_on(0, 1, 1, 60, 1.0)]);
    head.advance(8, 0).unwrap();
    let mut retuned = notes(super::tests::score(&[(0, 100, 1, 64)]), 8);
    retuned.carry(&mut same, head, 0).unwrap();
    retuned.prepare(head, 0, 8, false).unwrap();
    assert!(retuned.events().unwrap().is_empty());
    head.advance(8, 0).unwrap();
    let mut waiting = notes(retuned.score.clone(), 8);
    {
        let _guard = crate::ForbidAlloc::enter();
        waiting.carry(&mut retuned, head, 37).unwrap();
        waiting.prepare(head, 37, 8, false).unwrap();
    }
    assert_eq!(waiting.events().unwrap(), &[Event::note_off(0, 1, 1, 60)]);
    head.advance(8, 0).unwrap();
    waiting.prepare(head, 37, 8, true).unwrap();
    assert_eq!(
        waiting.events().unwrap(),
        &[Event::note_on(5, 1, 1, 64, 1.0)]
    );
    head.advance(8, 0).unwrap();
    let mut deleted = notes(super::tests::score(&[]), 8);
    {
        let _guard = crate::ForbidAlloc::enter();
        deleted.carry(&mut waiting, head, 37).unwrap();
        deleted.prepare(head, 37, 8, false).unwrap();
    }
    assert_eq!(deleted.events().unwrap(), &[Event::note_off(0, 1, 1, 64)]);
}

#[test]
fn exhausted_or_invalid_event_ranges_fail_without_panicking_or_allocating() {
    // Deliberately insufficient only for this regression; there is no product cap.
    let mut limited = Notes::new(score(&[(0, 64, 1, 60)]), 8, 0).unwrap();
    let head = Playhead::new(0, (None, None));
    let _guard = crate::ForbidAlloc::enter();
    assert!(matches!(
        limited.prepare(head, 0, 8, false),
        Err(PlaybackError::BufferTooSmall)
    ));
    limited.count = 1;
    limited.chased_count = 1;
    assert!(matches!(
        limited.events(),
        Err(PlaybackError::BufferTooSmall)
    ));
    assert!(matches!(
        limited.chased(),
        Err(PlaybackError::BufferTooSmall)
    ));
}
