//! Sample-accurate MIDI and metronome clicks for one block.
//!
//! Neither function allocates. The event slice and the sample buffer belong to
//! the caller; their lengths are scratch for this block, not a limit on how
//! many notes a project may contain.

use std::sync::Arc;

use echode_model::{Meter, TICKS_PER_QUARTER, TempoMap, Tick, Track};

use crate::clock::{sample_to_tick, tick_to_sample};
use crate::transport::{PlaybackError, PlayedSpan};

/// A note edge inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    pub sample_offset: u32,
    pub track: echode_model::Id,
    pub note: echode_model::Id,
    pub pitch: u8,
    pub velocity: u8,
    pub kind: MidiKind,
}

/// Note-off sorts before note-on at the same sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MidiKind {
    Off,
    On,
}

/// Writes note edges that fall in `spans`, then sorts them by time.
///
/// A span with [`PlayedSpan::release_at`] also stops notes that were still
/// sounding when the playhead jumped. The buffer is unspecified if this returns
/// an error.
pub fn schedule(
    tracks: &[Arc<Track>],
    tempo: &TempoMap,
    sample_rate: u32,
    spans: &[PlayedSpan],
    out: &mut [MidiEvent],
) -> Result<usize, PlaybackError> {
    let mut count = 0;
    for span in spans {
        let end = span
            .start_sample
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        for track in tracks {
            for clip in &track.clips {
                for note in &clip.notes {
                    let Some(start_tick) = clip.absolute_start(note) else {
                        return Err(PlaybackError::Time);
                    };
                    let end_tick = start_tick
                        .get()
                        .checked_add(note.length.get())
                        .ok_or(PlaybackError::Time)?;
                    let on_sample = tick_to_sample(start_tick, tempo, sample_rate)
                        .ok_or(PlaybackError::Time)?;
                    let off_sample = tick_to_sample(Tick::new(end_tick), tempo, sample_rate)
                        .ok_or(PlaybackError::Time)?;
                    if (span.start_sample..end).contains(&on_sample) {
                        push(
                            out,
                            &mut count,
                            event(span, on_sample, track, note, MidiKind::On)?,
                        )?;
                    }
                    if (span.start_sample..end).contains(&off_sample) {
                        push(
                            out,
                            &mut count,
                            event(span, off_sample, track, note, MidiKind::Off)?,
                        )?;
                    }
                    if let Some(release) = span.release_at
                        && on_sample < release
                        && off_sample >= release
                    {
                        push(
                            out,
                            &mut count,
                            MidiEvent {
                                sample_offset: span.block_offset,
                                track: track.id,
                                note: note.id,
                                pitch: note.pitch,
                                velocity: 0,
                                kind: MidiKind::Off,
                            },
                        )?;
                    }
                }
            }
        }
    }
    out[..count].sort_unstable_by(|left, right| {
        left.sample_offset
            .cmp(&right.sample_offset)
            .then(left.kind.cmp(&right.kind))
            .then(left.track.cmp(&right.track))
            .then(left.pitch.cmp(&right.pitch))
            .then(left.note.cmp(&right.note))
    });
    Ok(count)
}

fn event(
    span: &PlayedSpan,
    sample: i64,
    track: &Track,
    note: &echode_model::Note,
    kind: MidiKind,
) -> Result<MidiEvent, PlaybackError> {
    let local = u32::try_from(sample - span.start_sample).map_err(|_| PlaybackError::Time)?;
    Ok(MidiEvent {
        sample_offset: span
            .block_offset
            .checked_add(local)
            .ok_or(PlaybackError::Time)?,
        track: track.id,
        note: note.id,
        pitch: note.pitch,
        velocity: if kind == MidiKind::On {
            note.velocity
        } else {
            0
        },
        kind,
    })
}

fn push(out: &mut [MidiEvent], count: &mut usize, event: MidiEvent) -> Result<(), PlaybackError> {
    if *count >= out.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    out[*count] = event;
    *count += 1;
    Ok(())
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
    let bar = meter.bar_ticks().ok_or(PlaybackError::Time)?.get();
    for span in spans {
        let span_end = span
            .block_offset
            .checked_add(span.frames)
            .ok_or(PlaybackError::Time)?;
        if span_end as usize > output.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
        let start_tick = sample_to_tick(span.start_sample, tempo, sample_rate)
            .ok_or(PlaybackError::Time)?
            .get();
        let mut quarter = start_tick.div_euclid(TICKS_PER_QUARTER);
        let end_sample = span
            .start_sample
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        loop {
            let tick = quarter
                .checked_mul(TICKS_PER_QUARTER)
                .ok_or(PlaybackError::Time)?;
            let sample =
                tick_to_sample(Tick::new(tick), tempo, sample_rate).ok_or(PlaybackError::Time)?;
            if sample >= end_sample {
                break;
            }
            if sample >= span.start_sample {
                let local =
                    u32::try_from(sample - span.start_sample).map_err(|_| PlaybackError::Time)?;
                let offset = span.block_offset + local;
                let downbeat = tick.rem_euclid(bar) == 0;
                write_click(output, offset, downbeat, sample_rate);
            }
            quarter = quarter.checked_add(1).ok_or(PlaybackError::Time)?;
        }
    }
    Ok(())
}

fn write_click(output: &mut [f32], offset: u32, downbeat: bool, sample_rate: u32) {
    let length = (sample_rate / 200).max(1);
    let start = offset as usize;
    if start >= output.len() {
        return;
    }
    let count = (start + length as usize).min(output.len()) - start;
    for (index, sample) in output.iter_mut().skip(start).take(count).enumerate() {
        *sample += click_sample(downbeat, u32::try_from(index).unwrap_or(0), sample_rate);
    }
}

fn click_sample(downbeat: bool, sample_in_click: u32, sample_rate: u32) -> f32 {
    let frequency = if downbeat { 1_760.0 } else { 1_174.0 };
    let amplitude = if downbeat { 0.5 } else { 0.22 };
    let time = sample_in_click as f32 / sample_rate as f32;
    let decay = (-time * 80.0).exp();
    let phase = time * frequency * std::f32::consts::TAU;
    amplitude * decay * phase.cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::{Clip, ClipKind, Id, Note, Project, Tempo, Tick};

    fn project() -> Project {
        Project::try_assemble(
            Meter::four_four(),
            TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
            vec![Track {
                id: Id::from_raw(1),
                name: "piano".into(),
                parent: None,
                clips: vec![Clip {
                    id: Id::from_raw(2),
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                    kind: ClipKind::Fixed,
                    notes: vec![Note {
                        id: Id::from_raw(3),
                        start: Tick::new(0),
                        length: Tick::quarters(1).unwrap(),
                        pitch: 60,
                        velocity: 100,
                    }],
                }],
            }],
        )
        .unwrap()
    }

    fn span(start: i64, frames: u32) -> PlayedSpan {
        PlayedSpan {
            start_sample: start,
            frames,
            block_offset: 0,
            release_at: None,
        }
    }

    #[test]
    fn a_quarter_note_opens_at_the_downbeat_and_closes_half_a_second_later() {
        let project = project();
        let tempo = project.tempo().clone();
        let mut events = [MidiEvent {
            sample_offset: 0,
            track: Id::from_raw(0),
            note: Id::from_raw(0),
            pitch: 0,
            velocity: 0,
            kind: MidiKind::Off,
        }; 4];
        let count = schedule(
            project.tracks(),
            &tempo,
            48_000,
            &[span(0, 64)],
            &mut events,
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(events[0].kind, MidiKind::On);
        assert_eq!(events[0].sample_offset, 0);
        assert_eq!(events[0].pitch, 60);
        assert_eq!(events[0].velocity, 100);

        let count = schedule(
            project.tracks(),
            &tempo,
            48_000,
            &[span(24_000, 64)],
            &mut events,
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(events[0].kind, MidiKind::Off);
        assert_eq!(events[0].sample_offset, 0);
        assert_eq!(events[0].velocity, 0);
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
            },
        ];
        let mut events = [MidiEvent {
            sample_offset: 0,
            track: Id::from_raw(0),
            note: Id::from_raw(0),
            pitch: 0,
            velocity: 0,
            kind: MidiKind::Off,
        }; 8];
        let count = schedule(project.tracks(), &tempo, 48_000, &spans, &mut events).unwrap();
        assert_eq!(count, 3);
        assert_eq!(events[0].kind, MidiKind::On);
        assert_eq!(events[0].sample_offset, 0);
        assert_eq!(events[1].kind, MidiKind::Off);
        assert_eq!(events[1].sample_offset, 24_000);
        assert_eq!(events[2].kind, MidiKind::On);
        assert_eq!(events[2].sample_offset, 24_000);
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
}
