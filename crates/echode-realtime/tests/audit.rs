//! Open items from docs/reviews/2026-11-03-full-code-audit.md. Each test fails
//! until its item is fixed; `make known-bugs` runs them. Remove the `#[ignore]`
//! with the fix.

use echode_realtime::{Event, NoteSpan, Plan, Span, TrackPlan};

const SILENT: Event = Event::note_off(0, 0, 0, 0);

/// One note that starts before the loop and lasts past its end.
fn held_across_the_loop() -> Plan {
    Plan {
        tracks: vec![TrackPlan {
            track: 1,
            track_key: 0,
            notes: vec![NoteSpan {
                on: 0,
                off: 10_000,
                note: 7,
                note_id: 3,
                pitch: 60,
                velocity: 65_535,
            }],
            by_off: vec![0],
            gain: 1.0,
            pan: 0.0,
            mute: 0.0,
            solo: 0.0,
        }],
    }
}

fn span(start: i64, frames: u32, block_offset: u32, release_at: Option<i64>) -> Span {
    Span {
        start,
        frames,
        block_offset,
        release_at,
    }
}

/// FCA-01: a wrap visits every note that started before the loop end, sounding
/// or not, so its cost grows with the score (see `make spike-scale`). The
/// visible part is a second note-off for a note the first wrap already stopped.
/// The fix walks only the sounding notes; also bound the visits per block.
#[test]
fn audit_fca01_a_loop_wrap_releases_only_sounding_notes() {
    let plan = held_across_the_loop();
    let mut cursor = plan.cursor();
    let mut events = [SILENT; 8];
    let mut order = [0usize; 8];
    let mut scratch = [SILENT; 8];
    // Playback starts inside the loop [1_000, 2_000), so the long note is chased.
    cursor.catch_up(&plan, 1_000);
    let blocks: [&[Span]; 4] = [
        &[span(1_000, 600, 0, None)],
        &[
            span(1_600, 400, 0, None),
            span(1_000, 200, 400, Some(2_000)),
        ],
        &[span(1_200, 600, 0, None)],
        &[
            span(1_800, 200, 0, None),
            span(1_000, 400, 200, Some(2_000)),
        ],
    ];
    let mut offs = [0usize; 4];
    for (offs, spans) in offs.iter_mut().zip(blocks) {
        let count = cursor
            .play(&plan, spans, &mut events, &mut order, &mut scratch)
            .unwrap();
        *offs = events[..count]
            .iter()
            .filter(|event| event.is_note_off())
            .count();
    }
    // The first wrap stops the chased note. The wrap repositions without
    // chasing, so nothing is sounding at the second wrap.
    assert_eq!(offs[1], 1, "the first wrap stops the held note");
    assert_eq!(
        offs[3], 0,
        "the second wrap sent {} note-off(s) for a note that was not sounding",
        offs[3]
    );
}
