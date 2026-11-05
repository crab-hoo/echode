//! Builds a [`echode_realtime::Plan`] from a project snapshot.

use std::collections::HashMap;

use echode_model::{Project, TempoMap};
use echode_realtime::{NoteSpan, Plan, TempoIndex, TempoSegment, TrackPlan};

use crate::transport::PlaybackError;

pub fn tempo_index(tempo: &TempoMap) -> Result<TempoIndex, PlaybackError> {
    let points = tempo.points();
    let mut boundaries = Vec::with_capacity(points.len() + 1);
    if points.first().is_none_or(|point| point.at.get() != 0) {
        boundaries.push(0);
    }
    for point in points {
        if boundaries.last().copied() != Some(point.at.get()) {
            boundaries.push(point.at.get());
        }
    }
    let mut flick = 0i128;
    let mut segments = Vec::with_capacity(boundaries.len());
    for (index, start) in boundaries.iter().copied().enumerate() {
        let tempo = tempo_at(tempo, start);
        segments.push(TempoSegment {
            start_tick: start,
            start_flick: flick,
            numerator: tempo.numerator(),
            denominator: tempo.denominator(),
        });
        let end = boundaries.get(index + 1).copied().unwrap_or(start);
        if end > start {
            flick = flick
                .checked_add(flicks_between(
                    start,
                    end,
                    tempo.numerator(),
                    tempo.denominator(),
                )?)
                .ok_or(PlaybackError::Time)?;
        }
    }
    TempoIndex::new(segments).ok_or(PlaybackError::Time)
}

fn tempo_at(map: &TempoMap, tick: i64) -> echode_model::Tempo {
    let mut tempo = map.points()[0].tempo;
    for point in map.points() {
        if point.at.get() <= tick {
            tempo = point.tempo;
        } else {
            break;
        }
    }
    tempo
}

fn flicks_between(
    from: i64,
    to: i64,
    numerator: u32,
    denominator: u32,
) -> Result<i128, PlaybackError> {
    let ticks = i128::from(to) - i128::from(from);
    let scale = 60_000i128
        .checked_mul(i128::from(denominator))
        .ok_or(PlaybackError::Time)?;
    let numerator_product = ticks.checked_mul(scale).ok_or(PlaybackError::Time)?;
    div_round(numerator_product, i128::from(numerator)).ok_or(PlaybackError::Time)
}

fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    if denominator <= 0 {
        return None;
    }
    let half = denominator / 2;
    let adjusted = if numerator >= 0 {
        numerator.checked_add(half)?
    } else {
        numerator.checked_sub(half)?
    };
    Some(adjusted / denominator)
}

/// Note and track numbers for one playback. Built on the control thread.
///
/// Surviving model IDs keep their numbers across successful rebuilds. Removed
/// mappings are released on the control thread, but their numbers are never
/// reused while older plans may still be in flight.
#[derive(Debug, Default)]
pub struct PlaybackIds {
    notes: HashMap<u128, u32>,
    tracks: HashMap<u128, u32>,
    next_note: u32,
    next_track: u32,
}

impl PlaybackIds {
    pub fn new() -> Self {
        Self::default()
    }

    fn note(&mut self, id: u128) -> Result<u32, PlaybackError> {
        assign(&mut self.notes, &mut self.next_note, id)
    }

    fn track(&mut self, id: u128) -> Result<u32, PlaybackError> {
        assign(&mut self.tracks, &mut self.next_track, id)
    }
}

fn assign(ids: &mut HashMap<u128, u32>, next: &mut u32, model: u128) -> Result<u32, PlaybackError> {
    if let Some(id) = ids.get(&model).copied() {
        return Ok(id);
    }
    if *next >= i32::MAX as u32 {
        return Err(PlaybackError::IdSpace);
    }
    let id = *next;
    *next += 1;
    ids.insert(model, id);
    Ok(id)
}

pub fn build_plan(
    project: &Project,
    tempo: &TempoMap,
    sample_rate: u32,
    ids: &mut PlaybackIds,
) -> Result<Plan, PlaybackError> {
    let index = tempo_index(tempo)?;
    let mut tracks = Vec::with_capacity(project.tracks().len());
    let mut current_notes = HashMap::new();
    let mut current_tracks = HashMap::new();
    for track in project.tracks() {
        let track_key = ids.track(track.id.raw())?;
        current_tracks.insert(track.id.raw(), track_key);
        let mut notes = Vec::new();
        for clip in &track.clips {
            let Some(content) = project.content(clip.content) else {
                return Err(PlaybackError::Time);
            };
            for note in &content.notes {
                let Some((on, off)) = clip.audible(note).map_err(|_| PlaybackError::Time)? else {
                    continue;
                };
                let on = index
                    .tick_to_sample(on.get(), sample_rate)
                    .ok_or(PlaybackError::Time)?;
                let off = index
                    .tick_to_sample(off.get(), sample_rate)
                    .ok_or(PlaybackError::Time)?;
                if on >= off {
                    continue;
                }
                let note_id = ids.note(note.id.raw())?;
                current_notes.insert(note.id.raw(), note_id);
                notes.push(NoteSpan {
                    on,
                    off,
                    note: note.id.raw(),
                    note_id,
                    pitch: note.pitch,
                    velocity: note.velocity,
                });
            }
        }
        notes.sort_by(|left, right| {
            left.on
                .cmp(&right.on)
                .then(left.pitch.cmp(&right.pitch))
                .then(left.note.cmp(&right.note))
        });
        let mut by_off: Vec<u32> = (0..notes.len()).map(|index| index as u32).collect();
        by_off.sort_by(|left, right| {
            let left = &notes[*left as usize];
            let right = &notes[*right as usize];
            left.off
                .cmp(&right.off)
                .then(left.pitch.cmp(&right.pitch))
                .then(left.note.cmp(&right.note))
        });
        let mixer = mixer_values(track)?;
        tracks.push(TrackPlan {
            track: track.id.raw(),
            track_key,
            notes,
            by_off,
            gain: mixer.0,
            pan: mixer.1,
            mute: mixer.2,
            solo: mixer.3,
        });
    }
    // Publish only after the complete build succeeds. Dropping superseded hash
    // storage here also releases capacity left by large, since-deleted scores.
    ids.notes = current_notes;
    ids.tracks = current_tracks;
    Ok(Plan { tracks })
}

fn mixer_values(track: &echode_model::Track) -> Result<(f32, f32, f32, f32), PlaybackError> {
    use echode_model::{DeviceKind, ParamKey};
    let mixer = track
        .devices
        .iter()
        .find(|device| device.kind == DeviceKind::Mixer)
        .ok_or(PlaybackError::InvalidFader)?;
    let value = |key: ParamKey| {
        mixer
            .value(key)
            .filter(|value| key.allows(*value))
            .ok_or(PlaybackError::InvalidFader)
    };
    Ok((
        value(ParamKey::Gain)?,
        value(ParamKey::Pan)?,
        value(ParamKey::Mute)?,
        value(ParamKey::Solo)?,
    ))
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
