//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use echode_engine::{MidiEvent, MidiKind, PlayedSpan, schedule};
use echode_model::{Id, NoteDraft, Project, TICKS_PER_QUARTER, Tick};

#[test]
#[ignore = "MOD-5: notes outside a fixed clip still sound"]
fn audit_mod5_a_note_outside_a_fixed_clip_is_silent() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER / 2),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(
            track,
            clip,
            Tick::quarters(2).unwrap(),
            Tick::quarters(2).unwrap(),
        )
        .unwrap();
    let silent = MidiEvent {
        sample_offset: 0,
        track: Id::from_raw(0),
        note: Id::from_raw(0),
        pitch: 0,
        velocity: 0,
        kind: MidiKind::Off,
    };
    let mut events = [silent; 8];
    let span = PlayedSpan {
        start_sample: 0,
        frames: 128,
        block_offset: 0,
        release_at: None,
    };
    let count = schedule(
        project.tracks(),
        project.tempo(),
        48_000,
        &[span],
        &mut events,
    )
    .unwrap();
    assert_eq!(
        count, 0,
        "the clip now starts at beat 3, but {count} event(s) came out at beat 1"
    );
}
