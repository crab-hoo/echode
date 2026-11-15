use super::*;
use echode_model::{Tempo, Tick};

fn range(held: Option<i64>) -> RenderRange {
    RenderRange {
        loop_bounds: (None, None),
        started_before: held,
        trajectory: None,
        source_fade: None,
        meter_release: 1.0,
    }
}

fn snapshot(event: Event) -> Transport {
    let Event::Transport(event) = event else {
        panic!("not transport")
    };
    event
}

#[test]
fn transport_metadata_uses_both_timelines_and_euclidean_bars_under_rtsan() {
    let project = Project::with_signature(
        Meter::try_new(7, 8).unwrap(),
        Tempo::rational(120, 1).unwrap(),
    );
    let mut timeline = Timeline::new(
        &project,
        project.tempo().index(),
        48_000,
        64,
        Some((-24_000, 48_000)),
    )
    .unwrap();
    let events = timeline.stopped(-12_000).unwrap();
    let event = snapshot(events[0]);
    assert!(!event.playing && !event.recording && event.loop_active);
    assert_eq!(event.time, 0);
    assert_eq!(event.tempo_bpm, 120.0);
    assert_eq!(event.tempo_inc, 0.0);
    assert_eq!(event.song_pos_beats, -0.5);
    assert_eq!(event.song_pos_seconds, -0.25);
    assert_eq!((event.loop_start_beats, event.loop_end_beats), (-1.0, 2.0));
    assert_eq!(
        (event.loop_start_seconds, event.loop_end_seconds),
        (-0.5, 1.0)
    );
    assert_eq!((event.bar_start, event.bar_number), (-3.5, -1));
    assert_eq!(
        (event.signature_numerator, event.signature_denominator),
        (7, 8)
    );
    // An advancing source fade must not advertise musical playback after a stop.
    let spans = [Span {
        start: 99,
        frames: 32,
        block_offset: 0,
        release_at: None,
    }];
    assert_eq!(
        snapshot(timeline.fill(&spans, range(Some(-12_000))).unwrap()[0]),
        event
    );
}

#[test]
fn transport_density_bounds_cover_tempo_changes_and_short_loops_without_allocating_under_rtsan() {
    let mut project = Project::new();
    for tick in -20..100 {
        project = project.with_tempo(
            Tick::new(tick * 147),
            Tempo::rational(if tick % 2 == 0 { 120 } else { 60 }, 1).unwrap(),
        );
    }
    // These block shapes and sample positions exercise the capacity bound only.
    for frames in [1, 7, 32, 127] {
        for bounds in [None, Some((-5, -4)), Some((-5, 7)), Some((20, 25))] {
            let mut timeline =
                Timeline::new(&project, project.tempo().index(), 48_000, frames, bounds).unwrap();
            let mut spans = vec![
                Span {
                    start: 0,
                    frames: 0,
                    block_offset: 0,
                    release_at: None
                };
                frames
            ];
            for start in -30..50 {
                let (count, _) = crate::playback::split_loop(
                    start,
                    frames,
                    bounds.map(|b| b.0),
                    bounds.map(|b| b.1),
                    &mut spans,
                )
                .unwrap();
                let guard = crate::realtime::ForbidAlloc::enter();
                let events = timeline.fill(&spans[..count], range(None)).unwrap();
                assert!(!events.is_empty());
                assert_eq!(events[0].time(), 0);
                assert!(
                    events
                        .windows(2)
                        .all(|pair| pair[0].time() < pair[1].time())
                );
                assert!(events.iter().all(|event| (event.time() as usize) < frames));
                drop(guard);
                for latency in [0, 37, 131] {
                    let mut trajectory =
                        Playhead::new(start, (bounds.map(|b| b.0), bounds.map(|b| b.1)));
                    for _ in 0..3 {
                        let guard = crate::realtime::ForbidAlloc::enter();
                        let events = timeline.delayed(trajectory, latency, frames).unwrap();
                        assert_eq!(events[0].time(), 0);
                        assert!(
                            events
                                .windows(2)
                                .all(|pair| pair[0].time() < pair[1].time())
                        );
                        assert!(events.iter().all(|event| (event.time() as usize) < frames));
                        drop(guard);
                        trajectory.advance(frames, 0).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn collapsed_subsample_tempos_have_one_event_and_fractional_positions() {
    let project = Project::new()
        .with_tempo(Tick::new(-7), Tempo::rational(175, 3).unwrap())
        .with_tempo(Tick::new(1), Tempo::rational(250, 3).unwrap())
        .with_tempo(Tick::new(2), Tempo::rational(91, 1).unwrap());
    let mut timeline = Timeline::new(&project, project.tempo().index(), 96_000, 7, None).unwrap();
    let spans = [Span {
        start: -2,
        frames: 7,
        block_offset: 0,
        release_at: None,
    }];
    let events = timeline.fill(&spans, range(None)).unwrap();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].time() < pair[1].time())
    );
    let at_zero = snapshot(*events.iter().find(|event| event.time() == 2).unwrap());
    assert_eq!(at_zero.tempo_bpm, 91.0);
    assert!(!at_zero.loop_active);
    assert_eq!(at_zero.song_pos_seconds, 0.0);
    // The sample is just before the exact rational tempo boundary, not tick zero.
    assert!(at_zero.song_pos_beats.abs() < 1.0e-5);
}

#[test]
fn remote_tempo_points_do_not_refuse_playback_or_erase_subtick_positions() {
    let project = Project::new()
        .with_tempo(Tick::new(i64::MIN), Tempo::rational(120, 1).unwrap())
        .with_tempo(Tick::new(i64::MAX), Tempo::rational(1, u32::MAX).unwrap());
    let mut timeline = Timeline::new(&project, project.tempo().index(), 48_000, 64, None).unwrap();
    let event = snapshot(timeline.stopped(-1).unwrap()[0]);
    assert!((event.song_pos_beats + 1.0 / 24_000.0).abs() < 1.0e-12);
    assert_eq!(event.tempo_bpm, 120.0);
    let event = snapshot(timeline.stopped(1).unwrap()[0]);
    assert!((event.song_pos_beats - 1.0 / 24_000.0).abs() < 1.0e-12);
}
