//! Sample-accurate MIDI and metronome clicks for one block.
//!
//! Neither function allocates. The event slice and the sample buffer belong to
//! the caller; their lengths are scratch for this block, not a limit on how
//! many notes a project may contain.

use echode_model::{Meter, Project, TICKS_PER_QUARTER, TempoMap};
use echode_realtime::Event;

use crate::plan::PlaybackIds;
use crate::transport::{PlaybackError, PlayedSpan};

/// Writes note edges that fall in `spans`, then sorts them by time.
///
/// A span with [`PlayedSpan::release_at`] also stops notes that were still
/// sounding when the playhead jumped. The buffer is unspecified if this returns
/// an error.
pub fn schedule(
    project: &Project,
    tempo: &TempoMap,
    sample_rate: u32,
    spans: &[PlayedSpan],
    out: &mut [Event],
) -> Result<usize, PlaybackError> {
    let mut ids = PlaybackIds::new();
    let plan = crate::plan::build_plan(project, tempo, sample_rate, &mut ids)?;
    let mut cursor = plan.cursor();
    play_prepared(&plan, &mut cursor, spans, out)
}

pub(crate) fn play_prepared(
    plan: &echode_realtime::Plan,
    cursor: &mut echode_realtime::Cursor,
    spans: &[PlayedSpan],
    out: &mut [Event],
) -> Result<usize, PlaybackError> {
    let mut converted = [echode_realtime::Span {
        start: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
    }; 2];
    if spans.len() > converted.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    for (index, span) in spans.iter().enumerate() {
        converted[index] = to_span(*span);
    }
    cursor
        .play(plan, &converted[..spans.len()], out)
        .map_err(play_error)
}

fn to_span(span: PlayedSpan) -> echode_realtime::Span {
    echode_realtime::Span {
        start: span.start_sample,
        frames: span.frames,
        block_offset: span.block_offset,
        release_at: span.release_at,
    }
}

fn play_error(error: echode_realtime::PlayError) -> PlaybackError {
    match error {
        echode_realtime::PlayError::Time => PlaybackError::Time,
        echode_realtime::PlayError::BufferTooSmall => PlaybackError::BufferTooSmall,
    }
}

/// Adds metronome clicks into `output`. The buffer must already be silent.
///
/// A bar downbeat is louder and higher than the other quarter notes. The click
/// is a short decaying cosine so the first sample is the peak.
pub fn render_metronome(
    spans: &[PlayedSpan],
    tempo: &TempoMap,
    meter: Meter,
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    let index = crate::plan::tempo_index(tempo)?;
    let bar = meter.bar_ticks().ok_or(PlaybackError::Time)?.get();
    let length = i64::from((sample_rate / 200).max(1));
    for span in spans {
        let span_end = span
            .block_offset
            .checked_add(span.frames)
            .ok_or(PlaybackError::Time)?;
        if span_end as usize > output.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
        for local in 0..span.frames {
            let project = span
                .start_sample
                .checked_add(i64::from(local))
                .ok_or(PlaybackError::Time)?;
            let Some((downbeat, age)) = click_at(&index, project, bar, length, sample_rate)? else {
                continue;
            };
            output[(span.block_offset + local) as usize] +=
                click_sample(downbeat, age, sample_rate);
        }
    }
    Ok(())
}

fn click_at(
    index: &echode_realtime::TempoIndex,
    project: i64,
    bar: i64,
    length: i64,
    sample_rate: u32,
) -> Result<Option<(bool, u32)>, PlaybackError> {
    let tick = index
        .sample_to_tick(project, sample_rate)
        .ok_or(PlaybackError::Time)?;
    let quarter = tick.div_euclid(TICKS_PER_QUARTER);
    let earlier = quarter.checked_sub(1);
    let mut found = None;
    for candidate in [Some(quarter), earlier].into_iter().flatten() {
        let start_tick = candidate
            .checked_mul(TICKS_PER_QUARTER)
            .ok_or(PlaybackError::Time)?;
        let start = index
            .tick_to_sample(start_tick, sample_rate)
            .ok_or(PlaybackError::Time)?;
        if start <= project {
            let age = project - start;
            if age < length {
                let downbeat = start_tick.rem_euclid(bar) == 0;
                found = Some((downbeat, u32::try_from(age).unwrap_or(0)));
                break;
            }
        }
    }
    Ok(found)
}

fn click_sample(downbeat: bool, sample_in_click: u32, sample_rate: u32) -> f32 {
    let frequency = if downbeat { 1_760.0 } else { 1_174.0 };
    let amplitude = if downbeat { 0.5 } else { 0.22 };
    let time = sample_in_click as f32 / sample_rate as f32;
    let decay = libm::expf(-time * 80.0);
    let phase = time * frequency * std::f32::consts::TAU;
    amplitude * decay * libm::cosf(phase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::{
        ClipKind, Content, DEFAULT_FADE_FLICKS, Id, Lane, Note, Placement, Project, Tempo, Tick,
        Track,
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
        let started = std::time::Instant::now();
        let count = super::play_prepared(&plan, &mut cursor, &[span(0, 128)], &mut events).unwrap();
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
}
