//! Sample-accurate MIDI and metronome clicks for one block.
//!
//! `schedule` builds a plan and a sort workspace for callers that are not the
//! audio thread. The audio thread uses `play_prepared` with buffers the control
//! thread already owns. The metronome entry that takes a [`TempoMap`] builds
//! an index for callers without one; the audio thread uses the index prepared
//! with the plan. The event slice and the sample buffer belong to the caller;
//! their lengths are scratch for this block, not a limit on how many notes a
//! project may contain.

use echode_model::{Meter, Project, TICKS_PER_QUARTER, TempoMap};
use echode_realtime::{Event, TempoIndex};

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
    let silent = Event::note_off(0, 0, 0, 0);
    let mut order = vec![0usize; out.len()];
    let mut scratch = vec![silent; out.len()];
    play_prepared(&plan, &mut cursor, spans, out, &mut order, &mut scratch)
}

pub(crate) fn play_prepared(
    plan: &echode_realtime::Plan,
    cursor: &mut echode_realtime::Cursor,
    spans: &[PlayedSpan],
    out: &mut [Event],
    order: &mut [usize],
    scratch: &mut [Event],
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
        .play(plan, &converted[..spans.len()], out, order, scratch)
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
    render_metronome_index(
        spans,
        &crate::plan::tempo_index(tempo)?,
        meter,
        sample_rate,
        output,
    )
}

/// Same clicks as [`render_metronome`], using a tempo index built off the audio thread.
pub(crate) fn render_metronome_index(
    spans: &[PlayedSpan],
    index: &TempoIndex,
    meter: Meter,
    sample_rate: u32,
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    render_metronome_index_before(spans, index, meter, sample_rate, output, None)
}

pub(crate) fn render_metronome_index_before(
    spans: &[PlayedSpan],
    index: &TempoIndex,
    meter: Meter,
    sample_rate: u32,
    output: &mut [f32],
    cutoff: Option<i64>,
) -> Result<(), PlaybackError> {
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
            let Some((downbeat, age)) = click_at(index, project, bar, length, sample_rate, cutoff)?
            else {
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
    cutoff: Option<i64>,
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
        if start <= project && cutoff.is_none_or(|cutoff| start < cutoff) {
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
#[path = "sequence_tests.rs"]
mod tests;
