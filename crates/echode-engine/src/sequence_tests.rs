use super::*;
use echode_model::{
    ClipKind, Content, DEFAULT_FADE_FLICKS, Id, Lane, Note, Placement, Project, Tempo, Tick, Track,
};
use echode_realtime::Event;

fn quiet() -> Event {
    Event::note_off(0, 0, 0, 0)
}

fn unit(midi: u8) -> f64 {
    f64::from(echode_model::midi7_to_velocity(midi)) / 65535.0
}

fn project() -> Project {
    Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        vec![Content {
            id: Id::from_raw(4),
            audio: None,
            takes: Vec::new(),
            expressions: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(3),
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
                pitch: 60,
                velocity: echode_model::midi7_to_velocity(100),
                release: 0,
                articulation: None,
            }],
        }],
        vec![Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: echode_model::initial_order(),
            devices: vec![echode_model::test_mixer(1)],
            clips: vec![Placement {
                id: Id::from_raw(2),
                content: Id::from_raw(4),
                lane: Lane::Timeline,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
                offset: Tick::new(0),
                kind: ClipKind::Fixed,
                gain: 1.0,
                fade_in: DEFAULT_FADE_FLICKS,
                fade_out: DEFAULT_FADE_FLICKS,
            }],
        }],
    )
    .unwrap()
}

fn two_tracks() -> Project {
    let (skeleton, _) = Project::new().add_track("low");
    let (skeleton, _) = skeleton.add_track("high");
    let low_order = skeleton.tracks()[0].order.clone();
    let high_order = skeleton.tracks()[1].order.clone();
    let quarter = Tick::quarters(1).unwrap();
    Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        vec![
            Content {
                id: Id::from_raw(12),
                audio: None,
                takes: Vec::new(),
                expressions: Vec::new(),
                notes: vec![Note {
                    id: Id::from_raw(13),
                    start: Tick::new(0),
                    length: quarter,
                    pitch: 60,
                    velocity: echode_model::midi7_to_velocity(100),
                    release: 0,
                    articulation: None,
                }],
            },
            Content {
                id: Id::from_raw(22),
                audio: None,
                takes: Vec::new(),
                expressions: Vec::new(),
                notes: vec![Note {
                    id: Id::from_raw(23),
                    start: quarter,
                    length: quarter,
                    pitch: 64,
                    velocity: echode_model::midi7_to_velocity(80),
                    release: 0,
                    articulation: None,
                }],
            },
        ],
        vec![
            Track {
                id: Id::from_raw(10),
                name: "low".into(),
                parent: None,
                order: low_order,
                devices: vec![echode_model::test_mixer(10)],
                clips: vec![Placement {
                    id: Id::from_raw(11),
                    content: Id::from_raw(12),
                    lane: Lane::Timeline,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                    offset: Tick::new(0),
                    kind: ClipKind::Fixed,
                    gain: 1.0,
                    fade_in: DEFAULT_FADE_FLICKS,
                    fade_out: DEFAULT_FADE_FLICKS,
                }],
            },
            Track {
                id: Id::from_raw(20),
                name: "high".into(),
                parent: None,
                order: high_order,
                devices: vec![echode_model::test_mixer(20)],
                clips: vec![Placement {
                    id: Id::from_raw(21),
                    content: Id::from_raw(22),
                    lane: Lane::Timeline,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                    offset: Tick::new(0),
                    kind: ClipKind::Fixed,
                    gain: 1.0,
                    fade_in: DEFAULT_FADE_FLICKS,
                    fade_out: DEFAULT_FADE_FLICKS,
                }],
            },
        ],
    )
    .unwrap()
}

fn span(start: i64, frames: u32) -> PlayedSpan {
    PlayedSpan {
        start_sample: start,
        frames,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }
}

#[test]
fn a_quarter_note_opens_at_the_downbeat_and_closes_half_a_second_later() {
    let project = project();
    let tempo = project.tempo().clone();
    let mut events = [quiet(); 4];
    let count = schedule(&project, &tempo, 48_000, &[span(0, 64)], &mut events).unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_on());
    assert_eq!(events[0].time(), 0);
    assert_eq!(events[0].key(), Some(60));
    assert_eq!(events[0].velocity(), unit(100));

    let count = schedule(&project, &tempo, 48_000, &[span(24_000, 64)], &mut events).unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_off());
    assert_eq!(events[0].time(), 0);
    assert_eq!(events[0].velocity(), 0.0);
}

#[test]
fn wrapping_a_loop_releases_the_note_before_striking_it_again() {
    let project = project();
    let tempo = project.tempo().clone();
    let spans = [
        span(0, 24_000),
        PlayedSpan {
            start_sample: 0,
            frames: 64,
            block_offset: 24_000,
            release_at: Some(24_000),
            loop_end: None,
            wrap_from: None,
        },
    ];
    let mut events = [quiet(); 8];
    let count = schedule(&project, &tempo, 48_000, &spans, &mut events).unwrap();
    assert_eq!(count, 3);
    assert!(events[0].is_note_on());
    assert_eq!(events[0].time(), 0);
    assert!(events[1].is_note_off());
    assert_eq!(events[1].time(), 24_000);
    assert!(events[2].is_note_on());
    assert_eq!(events[2].time(), 24_000);
}

#[test]
fn a_note_off_in_the_middle_is_emitted_once() {
    let project = project();
    let tempo = project.tempo().clone();
    let mut events = [quiet(); 4];
    let spans = [span(24_000, 64), span(24_064, 64)];
    let count = schedule(&project, &tempo, 48_000, &spans, &mut events).unwrap();
    assert_eq!(count, 1);
    assert!(events[0].is_note_off());
    assert_eq!(events[0].time(), 0);
}

#[test]
fn edges_from_two_tracks_keep_off_before_on_and_track_order() {
    let project = two_tracks();
    let tempo = project.tempo().clone();
    let mut events = [quiet(); 4];
    let count = schedule(&project, &tempo, 48_000, &[span(24_000, 64)], &mut events).unwrap();
    assert_eq!(count, 2);
    assert!(events[0].is_note_off());
    assert_eq!(events[0].track_key(), Some(0));
    assert!(events[1].is_note_on());
    assert_eq!(events[1].track_key(), Some(1));
    assert_eq!(events[0].time(), events[1].time());
}

#[test]
fn the_downbeat_is_louder_than_the_next_quarter() {
    let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
    let mut downbeat = [0.0; 64];
    render_metronome(
        &[span(0, 64)],
        &tempo,
        Meter::four_four(),
        48_000,
        &mut downbeat,
    )
    .unwrap();
    let mut weak = [0.0; 64];
    render_metronome(
        &[span(24_000, 64)],
        &tempo,
        Meter::four_four(),
        48_000,
        &mut weak,
    )
    .unwrap();
    let mut between = [0.0; 64];
    render_metronome(
        &[span(1_000, 64)],
        &tempo,
        Meter::four_four(),
        48_000,
        &mut between,
    )
    .unwrap();
    assert!(downbeat[0] > weak[0]);
    assert!(weak[0] > 0.1);
    assert!(between.iter().all(|sample| *sample == 0.0));
}

#[test]
#[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
fn a_hundred_thousand_notes_schedule_one_block_under_the_engine_budget() {
    const NOTES: usize = 100_000;
    let mut notes = Vec::with_capacity(NOTES);
    for index in 0..NOTES {
        notes.push(Note {
            id: Id::from_raw(1_000_000 + index as u128),
            start: Tick::new(index as i64 * TICKS_PER_QUARTER),
            length: Tick::new(1),
            pitch: 60,
            velocity: echode_model::midi7_to_velocity(100),
            release: 0,
            articulation: None,
        });
    }
    let project = Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        vec![Content {
            id: Id::from_raw(3),
            audio: None,
            takes: Vec::new(),
            expressions: Vec::new(),
            notes,
        }],
        vec![Track {
            id: Id::from_raw(1),
            name: "piano".into(),
            parent: None,
            order: echode_model::initial_order(),
            devices: vec![echode_model::test_mixer(1)],
            clips: vec![Placement {
                id: Id::from_raw(2),
                content: Id::from_raw(3),
                lane: Lane::Timeline,
                start: Tick::new(0),
                length: Tick::new(NOTES as i64 * TICKS_PER_QUARTER),
                offset: Tick::new(0),
                kind: ClipKind::Fixed,
                gain: 1.0,
                fade_in: DEFAULT_FADE_FLICKS,
                fade_out: DEFAULT_FADE_FLICKS,
            }],
        }],
    )
    .expect("assemble");
    let tempo = project.tempo().clone();
    let mut ids = crate::plan::PlaybackIds::new();
    let plan = crate::plan::build_plan(&project, &tempo, 48_000, &mut ids).unwrap();
    let mut cursor = plan.cursor();
    let mut events = vec![quiet(); 8];
    let mut order = vec![0usize; events.len()];
    let mut scratch = vec![quiet(); events.len()];
    let started = std::time::Instant::now();
    let count = super::play_prepared(
        &plan,
        &mut cursor,
        &[span(0, 128)],
        &mut events,
        &mut order,
        &mut scratch,
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert!(
        count < 8,
        "the first block should hold only a few edges, held {count}"
    );
    println!(
        "100k notes, one 128-frame block: {} µs",
        elapsed.as_micros()
    );
    assert!(
        elapsed < std::time::Duration::from_micros(267),
        "block took {elapsed:?}"
    );
}

fn same_start_tracks(count: usize) -> Project {
    let mut skeleton = Project::new();
    for index in 0..count {
        skeleton = skeleton.add_track(format!("t{index}")).0;
    }
    let orders: Vec<_> = skeleton
        .tracks()
        .iter()
        .map(|track| track.order.clone())
        .collect();
    let quarter = Tick::quarters(1).unwrap();
    let contents = (0..count)
        .map(|index| Content {
            id: Id::from_raw(1_000 + index as u128),
            audio: None,
            takes: Vec::new(),
            expressions: Vec::new(),
            notes: vec![Note {
                id: Id::from_raw(2_000 + index as u128),
                start: Tick::new(0),
                length: quarter,
                pitch: 60,
                velocity: echode_model::midi7_to_velocity(100),
                release: 0,
                articulation: None,
            }],
        })
        .collect();
    let tracks = (0..count)
        .map(|index| Track {
            id: Id::from_raw(3_000 + index as u128),
            name: format!("t{index}"),
            parent: None,
            order: orders[index].clone(),
            devices: vec![echode_model::test_mixer(3_000 + index as u128)],
            clips: vec![Placement {
                id: Id::from_raw(4_000 + index as u128),
                content: Id::from_raw(1_000 + index as u128),
                lane: Lane::Timeline,
                start: Tick::new(0),
                length: Tick::quarters(4).unwrap(),
                offset: Tick::new(0),
                kind: ClipKind::Fixed,
                gain: 1.0,
                fade_in: DEFAULT_FADE_FLICKS,
                fade_out: DEFAULT_FADE_FLICKS,
            }],
        })
        .collect();
    Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        contents,
        tracks,
    )
    .unwrap()
}

#[test]
fn same_time_notes_on_many_tracks_stay_in_track_order() {
    let project = same_start_tracks(8);
    let tempo = project.tempo().clone();
    let mut events = vec![quiet(); 16];
    let count = schedule(&project, &tempo, 48_000, &[span(0, 64)], &mut events).unwrap();
    assert_eq!(count, 8);
    let scheduled = events[..count].to_vec();
    for (index, event) in scheduled.iter().enumerate() {
        assert!(event.is_note_on());
        assert_eq!(event.time(), 0);
        assert_eq!(event.track_key(), Some(index as u32));
    }
    let mut reversed = scheduled.clone();
    reversed.reverse();
    let mut order = vec![0usize; reversed.len()];
    let mut scratch = vec![quiet(); reversed.len()];
    assert!(echode_realtime::sort_events(
        &mut reversed,
        &mut order,
        &mut scratch
    ));
    assert_eq!(reversed, scheduled);
}

#[test]
#[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
fn reverse_same_time_events_sort_under_one_block() {
    // 128 frames at 48 kHz is 2.667 ms. Insertion sort took 270 ms at 8,000 events.
    for size in [1_000usize, 2_000, 4_000, 8_000] {
        let median = median_reverse_sort(size);
        println!(
            "reverse same-time sort of {size}: {} µs",
            median.as_micros()
        );
        if size == 8_000 {
            assert!(
                median < std::time::Duration::from_micros(2_667),
                "8,000 events took {median:?}"
            );
        }
    }
    let project = same_start_tracks(64);
    let tempo = project.tempo().clone();
    let mut ids = crate::plan::PlaybackIds::new();
    let plan = crate::plan::build_plan(&project, &tempo, 48_000, &mut ids).unwrap();
    let mut cursor = plan.cursor();
    let mut events = vec![quiet(); 128];
    let mut order = vec![0usize; events.len()];
    let mut scratch = vec![quiet(); events.len()];
    let started = std::time::Instant::now();
    let count = super::play_prepared(
        &plan,
        &mut cursor,
        &[span(0, 128)],
        &mut events,
        &mut order,
        &mut scratch,
    )
    .unwrap();
    let elapsed = started.elapsed();
    println!(
        "64 same-time tracks, one 128-frame block: {} µs, {count} events",
        elapsed.as_micros()
    );
    assert_eq!(count, 64);
    assert!(
        events[..count]
            .windows(2)
            .all(|pair| { pair[0].track_key().unwrap_or(0) <= pair[1].track_key().unwrap_or(0) })
    );
}

fn median_reverse_sort(count: usize) -> std::time::Duration {
    let mut events: Vec<_> = (0..count)
        .rev()
        .map(|index| Event::note_on(0, index as u32, index as u32, 60, 0.5))
        .collect();
    let mut order = vec![0usize; count];
    let mut scratch = vec![quiet(); count];
    assert!(echode_realtime::sort_events(
        &mut events,
        &mut order,
        &mut scratch
    ));
    assert_eq!(events.first().map(|event| event.track_key()), Some(Some(0)));
    assert_eq!(
        events.last().map(|event| event.track_key()),
        Some(Some((count - 1) as u32))
    );
    let mut samples = [std::time::Duration::ZERO; 3];
    for sample in &mut samples {
        events.reverse();
        let started = std::time::Instant::now();
        assert!(echode_realtime::sort_events(
            &mut events,
            &mut order,
            &mut scratch
        ));
        *sample = started.elapsed();
    }
    samples.sort();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].track_key() <= pair[1].track_key())
    );
    samples[1]
}
