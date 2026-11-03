// Fixture construction and assertions run off the audio thread.
#![allow(clippy::disallowed_methods)]
use super::*;

const SILENT: Event = Event::note_off(0, 0, 0, 0);

fn score(count: usize) -> Plan {
    // Only this fixture is bounded. One short note per ten samples, followed
    // by two overlapping voices; model IDs run opposite to playback order.
    let mut notes: Vec<_> = (0..count)
        .map(|index| NoteSpan {
            on: index as i64 * 10,
            off: index as i64 * 10 + 2,
            note: (count - index) as u128,
            note_id: index as u32,
            pitch: 60,
            velocity: 65_535,
        })
        .collect();
    for index in 0..2 {
        notes.push(NoteSpan {
            on: count as i64 * 10,
            off: count as i64 * 10 + 100,
            note: count as u128 + 1 + index,
            note_id: count as u32 + index as u32,
            pitch: 64 + index as u8,
            velocity: 65_535,
        });
    }
    Plan {
        tracks: vec![TrackPlan {
            track: 51,
            track_key: 7,
            by_off: (0..notes.len() as u32).collect(),
            notes,
            gain: 1.0,
            pan: 0.0,
            mute: 0.0,
            solo: 0.0,
        }],
    }
}

fn play(cursor: &mut Cursor, plan: &Plan, span: Span) -> Vec<Event> {
    // Scratch is limited to this fixture's edges, not product polyphony.
    let mut events = vec![SILENT; plan.tracks[0].notes.len() * 2];
    let mut order = vec![0; events.len()];
    let mut scratch = vec![SILENT; events.len()];
    let count = cursor
        .play(plan, &[span], &mut events, &mut order, &mut scratch)
        .unwrap();
    events.truncate(count);
    events
}

fn visits(action: impl FnOnce()) -> usize {
    NOTE_VISITS.with(|counter| counter.set(0));
    action();
    NOTE_VISITS.with(|counter| counter.get())
}

fn span(start: i64, frames: u32) -> Span {
    Span {
        start,
        frames,
        block_offset: 0,
        release_at: None,
    }
}

#[test]
fn loop_wrap_work_follows_live_voices_not_the_score() {
    let mut work = Vec::new();
    for count in [64, 1_024] {
        let plan = score(count);
        let mut cursor = plan.cursor();
        let end = count as i64 * 10 + 50;
        play(&mut cursor, &plan, span(0, end as u32));
        work.push(visits(|| {
            let events = play(
                &mut cursor,
                &plan,
                Span {
                    release_at: Some(end),
                    ..span(end - 10, 1)
                },
            );
            assert_eq!(events.len(), 2);
            assert!(events.iter().all(|event| event.is_note_off()));
        }));
    }
    assert_eq!(
        work[0], work[1],
        "loop wrap inspected score-sized note history: {work:?}"
    );
    assert!(
        work[0] <= 4,
        "loop wrap visited more than the live voices: {work:?}"
    );
}

#[test]
fn edit_adoption_work_follows_live_voices_not_the_score() {
    let mut work = Vec::new();
    for count in [64, 1_024] {
        let plan = score(count);
        let mut previous = plan.cursor();
        let end = count as i64 * 10 + 50;
        play(&mut previous, &plan, span(0, end as u32));
        let next = score(count);
        let mut cursor = next.cursor();
        let mut events = [SILENT; 4];
        work.push(visits(|| {
            assert_eq!(
                previous
                    .release_replaced(&plan, &next, &cursor, end, 0, &mut events)
                    .unwrap(),
                0
            );
            cursor.catch_up(&next, end);
            cursor.keep_live_keys(&next, &previous, &plan);
        }));
    }
    assert_eq!(
        work[0], work[1],
        "edit adoption inspected score-sized note history: {work:?}"
    );
    assert!(
        work[0] <= 16,
        "edit adoption visited more than the live voices: {work:?}"
    );
}

#[test]
fn locate_work_follows_live_voices_not_the_score() {
    let mut work = Vec::new();
    for count in [64, 1_024] {
        let plan = score(count);
        let mut cursor = plan.cursor();
        let end = count as i64 * 10 + 50;
        play(&mut cursor, &plan, span(0, end as u32));
        let mut events = [SILENT; 4];
        work.push(visits(|| {
            assert_eq!(cursor.release_all(&plan, 0, &mut events).unwrap(), 2);
            cursor.catch_up(&plan, end - 1);
        }));
    }
    assert_eq!(
        work[0], work[1],
        "locate inspected score-sized note history: {work:?}"
    );
    assert!(
        work[0] <= 8,
        "locate visited more than the live voices: {work:?}"
    );
}

#[test]
fn clearing_a_voice_keeps_the_dense_set_and_reverse_slots_in_sync() {
    let mut plan = score(0);
    let mut middle = plan.tracks[0].notes[0];
    middle.note = 99;
    middle.note_id = 99;
    middle.off = 2;
    plan.tracks[0].notes.insert(1, middle);
    plan.tracks[0].by_off = vec![1, 0, 2];
    let mut cursor = plan.cursor();
    play(&mut cursor, &plan, span(0, 10));
    let track = &mut cursor.tracks[0];
    assert_eq!(track.sounding_len, 2);
    for slot in 0..track.sounding_len {
        assert_eq!(track.sounding_slots[track.sounding[slot] as usize], slot);
    }
    track.clear(1);
    assert_eq!(track.sounding_len, 2, "a second clear must be harmless");
    let events = play(&mut cursor, &plan, span(10, 100));
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.is_note_off()));
    assert_eq!(cursor.tracks[0].sounding_len, 0);
    assert!(
        cursor.tracks[0]
            .sounding_slots
            .iter()
            .all(|slot| *slot == usize::MAX)
    );
}

#[test]
fn indexed_replacement_handles_reordered_tracks_removed_notes_and_old_keys() {
    let mut plan = score(0);
    let mut other = plan.tracks[0].clone();
    other.track = 12;
    other.track_key = 3;
    plan.tracks.push(other);
    let mut previous = plan.cursor();
    play(&mut previous, &plan, span(0, 10));
    let mut next = plan.clone();
    next.tracks.reverse();
    next.tracks[1].notes[0].pitch = 72;
    next.tracks[0].notes.remove(0);
    next.tracks[0].by_off = vec![0];
    let mut cursor = next.cursor();
    let mut events = [SILENT; 8];
    let count = previous
        .release_replaced(&plan, &next, &cursor, 10, 4, &mut events)
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(events[0], Event::note_off(4, 3, 0, 64));
    cursor.catch_up(&next, 10);
    cursor.keep_live_keys(&next, &previous, &plan);
    let count = cursor.release_all(&next, 5, &mut events).unwrap();
    assert_eq!(count, 3);
    assert!(events[..count].contains(&Event::note_off(5, 7, 0, 64)));
    assert!(events[..count].contains(&Event::note_off(5, 7, 1, 65)));
    assert!(events[..count].contains(&Event::note_off(5, 3, 1, 65)));
    assert_eq!(
        cursor.release_all(&next, 0, &mut []),
        Err(PlayError::BufferTooSmall)
    );
}

#[test]
fn a_repeated_placement_looks_up_the_current_span_not_the_first_model_id() {
    let mut plan = score(0);
    plan.tracks[0].notes[0].off = 5;
    plan.tracks[0].notes[1].on = 10;
    plan.tracks[0].notes[1].note = plan.tracks[0].notes[0].note;
    let mut previous = plan.cursor();
    play(&mut previous, &plan, span(0, 20));
    let mut next = plan.clone();
    next.tracks[0].notes[1].pitch = 80;
    let mut cursor = next.cursor();
    let mut events = [SILENT; 4];
    assert_eq!(
        previous
            .release_replaced(&plan, &next, &cursor, 20, 0, &mut events)
            .unwrap(),
        0
    );
    cursor.catch_up(&next, 20);
    cursor.keep_live_keys(&next, &previous, &plan);
    assert_eq!(cursor.release_all(&next, 0, &mut events).unwrap(), 1);
    assert_eq!(events[0], Event::note_off(0, 7, 1, 65));
}
