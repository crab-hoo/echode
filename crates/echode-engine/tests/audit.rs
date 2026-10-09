//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use echode_engine::{Event, PlayedSpan, schedule};
use echode_model::{NoteDraft, Project, TICKS_PER_QUARTER, Tick};

#[test]
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
    let silent = Event::note_off(0, 0, 0, 0);
    let mut events = [silent; 8];
    let span = PlayedSpan {
        start_sample: 0,
        frames: 128,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    };
    let count = schedule(&project, project.tempo(), 48_000, &[span], &mut events).unwrap();
    assert_eq!(
        count, 0,
        "the clip now starts at beat 3, but {count} event(s) came out at beat 1"
    );
}

#[test]
fn a_note_that_crosses_the_window_ends_at_the_window() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip, Tick::new(0), Tick::quarters(1).unwrap())
        .unwrap();
    let tempo = project.tempo().clone();
    let mut events = [silent(); 8];
    let count = schedule(&project, &tempo, 48_000, &[span_at(0, 64)], &mut events).unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_on());
    assert_eq!(events[0].time(), 0);
    let count = schedule(
        &project,
        &tempo,
        48_000,
        &[span_at(24_000, 64)],
        &mut events,
    )
    .unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_off());
    assert_eq!(events[0].time(), 0);
    let count = schedule(
        &project,
        &tempo,
        48_000,
        &[span_at(96_000, 64)],
        &mut events,
    )
    .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_note_that_starts_before_the_window_opens_at_the_window() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(
            track,
            clip,
            Tick::quarters(1).unwrap(),
            Tick::quarters(1).unwrap(),
        )
        .unwrap();
    let tempo = project.tempo().clone();
    let mut events = [silent(); 8];
    let count = schedule(&project, &tempo, 48_000, &[span_at(0, 64)], &mut events).unwrap();
    assert_eq!(count, 0);
    let count = schedule(
        &project,
        &tempo,
        48_000,
        &[span_at(24_000, 64)],
        &mut events,
    )
    .unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_on());
    assert_eq!(events[0].time(), 0);
    let count = schedule(
        &project,
        &tempo,
        48_000,
        &[span_at(48_000, 64)],
        &mut events,
    )
    .unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_off());
    assert_eq!(events[0].time(), 0);
}

fn silent() -> Event {
    Event::note_off(0, 0, 0, 0)
}

fn span_at(start: i64, frames: u32) -> PlayedSpan {
    PlayedSpan {
        start_sample: start,
        frames,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }
}
