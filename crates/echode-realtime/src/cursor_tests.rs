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
                cursor
                    .take_over(&next, &mut previous, &plan, end, true, &mut events)
                    .unwrap(),
                0
            );
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
    let count = cursor
        .take_over(&next, &mut previous, &plan, 10, true, &mut events)
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(events[0], Event::note_off(0, 3, 0, 64));
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
        cursor
            .take_over(&next, &mut previous, &plan, 20, true, &mut events)
            .unwrap(),
        0
    );
    assert_eq!(cursor.release_all(&next, 0, &mut events).unwrap(), 1);
    assert_eq!(events[0], Event::note_off(0, 7, 1, 65));
}

/// Deterministic generator for the randomized adoption regression.
struct Steps(u64);

impl Steps {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }
}

/// Random overlapping notes on `[0, 2_000)`. Some model IDs repeat, as when
/// one content is placed twice.
fn random_track(steps: &mut Steps, track: u128, track_key: u32, first_id: u32) -> TrackPlan {
    // The note count bounds this fixture only.
    let count = 1 + steps.next(40) as usize;
    let mut notes: Vec<_> = (0..count)
        .map(|index| {
            let on = steps.next(1_900) as i64;
            NoteSpan {
                on,
                off: on + 1 + steps.next(300) as i64,
                note: u128::from(first_id) + steps.next(count as u64 - index as u64 / 2) as u128,
                note_id: first_id + index as u32,
                pitch: 40 + steps.next(40) as u8,
                velocity: 65_535,
            }
        })
        .collect();
    notes.sort_by_key(|note| (note.on, note.pitch, note.note));
    let mut by_off: Vec<u32> = (0..notes.len() as u32).collect();
    by_off.sort_by_key(|index| {
        let note = &notes[*index as usize];
        (note.off, note.pitch, note.note)
    });
    TrackPlan {
        track,
        track_key,
        notes,
        by_off,
        gain: 1.0,
        pan: 0.0,
        mute: 0.0,
        solo: 0.0,
    }
}

fn indices(cursor: &Cursor) -> Vec<Arc<NoteIndex>> {
    cursor
        .tracks
        .iter()
        .map(|track| Arc::clone(&track.index))
        .collect()
}

fn play_spans(cursor: &mut Cursor, plan: &Plan, spans: &[Span]) -> Vec<Event> {
    // Scratch is limited to this fixture's notes, not product polyphony.
    let room: usize = plan.tracks.iter().map(|track| track.notes.len()).sum();
    let mut events = vec![SILENT; room * 4 + 4];
    let mut order = vec![0; events.len()];
    let mut scratch = vec![SILENT; events.len()];
    let count = cursor
        .play(plan, spans, &mut events, &mut order, &mut scratch)
        .unwrap();
    events.truncate(count);
    events
}

fn of_track(events: &[Event], key: u32) -> Vec<Event> {
    events
        .iter()
        .copied()
        .filter(|event| event.track_key() == Some(key))
        .collect()
}

#[test]
fn an_unchanged_track_plays_as_if_no_plan_had_been_adopted() {
    let (mut handed_over, mut replaced, mut wraps) = (0, 0, 0);
    // The seed and round counts bound this regression's run time.
    for seed in 1..=40u64 {
        let mut steps = Steps(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut plan = Plan {
            tracks: (0..3)
                .map(|index| {
                    random_track(&mut steps, 100 + index, index as u32, index as u32 * 1_000)
                })
                .collect(),
        };
        // Track key 0 is never edited. Its reference never adopts a plan.
        let reference_plan = plan.clone();
        let mut reference = reference_plan.cursor();
        let mut cursor = plan.cursor();
        let (loop_start, loop_end) = (200 + steps.next(400) as i64, 1_200 + steps.next(600) as i64);
        let mut position = 0i64;
        reference.catch_up(&reference_plan, position);
        cursor.catch_up(&plan, position);
        for _ in 0..60 {
            let frames = 1 + steps.next(96) as u32;
            let spans = if position + i64::from(frames) > loop_end && position < loop_end {
                wraps += 1;
                let first = (loop_end - position) as u32;
                vec![
                    span(position, first),
                    Span {
                        start: loop_start,
                        frames: frames - first,
                        block_offset: first,
                        release_at: Some(loop_end),
                    },
                ]
            } else {
                vec![span(position, frames)]
            };
            let expected = of_track(&play_spans(&mut reference, &reference_plan, &spans), 0);
            let played = play_spans(&mut cursor, &plan, &spans);
            assert_eq!(of_track(&played, 0), expected, "seed {seed}");
            let last = spans.last().unwrap();
            position = last.start + i64::from(last.frames);
            if steps.next(3) != 0 {
                continue;
            }
            // Edit track key 1 or 2, sharing the other tracks' indexes as the
            // engine does for unchanged track plans.
            let edited = 1 + steps.next(2) as usize;
            let mut next = plan.clone();
            let shared = indices(&cursor);
            let mut lookups = Vec::new();
            for (slot, track) in next.tracks.iter_mut().enumerate() {
                if slot == edited {
                    if let Some(note) = track.notes.first_mut() {
                        note.pitch = note.pitch.wrapping_add(1) % 128;
                    }
                    if steps.next(2) == 0 && !track.notes.is_empty() {
                        let removed = steps.next(track.notes.len() as u64) as u32;
                        track.notes.remove(removed as usize);
                        track.by_off.retain(|index| *index != removed);
                        for index in &mut track.by_off {
                            *index -= u32::from(*index > removed);
                        }
                    }
                    lookups.push(Arc::new(NoteIndex::new(track)));
                } else {
                    lookups.push(Arc::clone(&shared[slot]));
                }
            }
            let mut adopted = next.cursor_with(lookups).unwrap();
            let mut offs = vec![SILENT; 64];
            let count = adopted
                .take_over(&next, &mut cursor, &plan, position, true, &mut offs)
                .unwrap();
            assert!(
                of_track(&offs[..count], 0).is_empty(),
                "seed {seed}: an unchanged track was released"
            );
            let (mut fresh, mut inherited) = (vec![SILENT; 64], vec![SILENT; 64]);
            let (struck, kept) = adopted
                .chase_events(&next, &mut fresh, &mut inherited)
                .unwrap();
            assert!(
                of_track(&fresh[..struck], 0).is_empty(),
                "seed {seed}: an unchanged track retriggered its voices"
            );
            handed_over += of_track(&inherited[..kept], 0).len();
            replaced += count + of_track(&inherited[..kept], edited as u32).len();
            cursor = adopted;
            plan = next;
        }
    }
    // The regression must reach held voices, released edits and loop wraps.
    assert!(handed_over > 20, "too few inherited voices: {handed_over}");
    assert!(replaced > 20, "too few edited voices: {replaced}");
    assert!(wraps > 20, "too few loop wraps: {wraps}");
}

#[test]
fn handing_over_an_unchanged_track_visits_no_notes() {
    for count in [64, 1_024] {
        let plan = score(count);
        let mut previous = plan.cursor();
        let end = count as i64 * 10 + 50;
        play(&mut previous, &plan, span(0, end as u32));
        let mut cursor = plan.cursor_with(indices(&previous)).unwrap();
        let mut offs = [SILENT; 4];
        let work = visits(|| {
            assert_eq!(
                cursor
                    .take_over(&plan, &mut previous, &plan, end, true, &mut offs)
                    .unwrap(),
                0
            );
        });
        assert_eq!(work, 0, "an unchanged track was searched again");
        let (mut fresh, mut inherited) = ([SILENT; 4], [SILENT; 4]);
        assert_eq!(
            cursor
                .chase_events(&plan, &mut fresh, &mut inherited)
                .unwrap(),
            (0, 2),
            "both held voices belong to the retained instrument"
        );
        // Not delivered: the same voices are struck on every instrument.
        let mut undelivered = plan.cursor_with(indices(&cursor)).unwrap();
        undelivered
            .take_over(&plan, &mut cursor, &plan, end, false, &mut offs)
            .unwrap();
        assert_eq!(
            undelivered
                .chase_events(&plan, &mut fresh, &mut inherited)
                .unwrap(),
            (2, 0)
        );
    }
}

#[test]
fn a_take_over_without_room_for_its_offs_leaves_both_cursors() {
    let plan = score(0);
    let mut previous = plan.cursor();
    play(&mut previous, &plan, span(0, 10));
    let mut next = plan.clone();
    next.tracks[0].notes.truncate(1);
    next.tracks[0].by_off = vec![0];
    let mut cursor = next.cursor();
    let reference = previous.clone();
    assert_eq!(
        cursor.take_over(&next, &mut previous, &plan, 10, true, &mut []),
        Err(PlayError::BufferTooSmall)
    );
    let (mut kept, mut expected) = (previous, reference);
    assert_eq!(
        play(&mut kept, &plan, span(10, 200)),
        play(&mut expected, &plan, span(10, 200)),
        "a failed take-over changed the previous cursor"
    );
    assert_eq!(
        cursor.position, None,
        "a failed take-over placed the next cursor"
    );
}
