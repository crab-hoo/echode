//! Builds a [`echode_realtime::Plan`] from a project snapshot.

use std::collections::HashMap;
use std::sync::Arc;

use echode_model::{Content, Project, TempoMap, Track};
use echode_realtime::{NoteIndex, NoteSpan, Plan, TempoIndex, TrackPlan};

use crate::PlaybackError;

pub fn tempo_index(tempo: &TempoMap) -> Result<TempoIndex, PlaybackError> {
    Ok(tempo.index().clone())
}

/// Note and track numbers for one playback, and the last plan of each track.
/// Built on the control thread.
///
/// Surviving model IDs keep their numbers across successful rebuilds. Removed
/// mappings are released on the control thread, but their numbers are never
/// reused while older plans may still be in flight.
///
/// A track whose model, clip contents, tempo map and rate are unchanged since
/// the last successful build keeps its plan and derived lookups, so an edit
/// replans only the tracks it touched (FCA-04).
#[derive(Debug, Default)]
pub struct PlaybackIds {
    notes: HashMap<u128, NoteNumber>,
    tracks: HashMap<u128, u32>,
    next_note: u32,
    next_track: u32,
    planned: HashMap<u128, PlannedTrack>,
    /// The tempo map and rate every entry of `planned` was built for.
    basis: Option<(TempoMap, u32)>,
    /// Note spans the last build computed rather than reused.
    #[cfg(test)]
    pub(crate) planned_notes: usize,
    /// Notes whose derived lookups or event demand were computed since that build.
    #[cfg(test)]
    pub(crate) derived_notes: usize,
}

/// A model note's playback number and how many current note spans use it.
/// One content placed twice gives two spans with the same model ID.
#[derive(Clone, Copy, Debug)]
struct NoteNumber {
    id: u32,
    uses: usize,
}

/// One track's plan, kept while its inputs are the same `Arc`s. The entry
/// holds them, so an identity compare never sees a reused address.
#[derive(Debug)]
struct PlannedTrack {
    track: Arc<Track>,
    contents: Vec<Arc<Content>>,
    plan: Arc<TrackPlan>,
    derived: TrackDerived,
    demand: Option<CachedDemand>,
    /// Computed by the build that stored it, rather than carried over.
    rebuilt: bool,
}

/// A track's event demand and the loop bounds and block frames it was sized for.
#[derive(Clone, Copy, Debug)]
struct CachedDemand {
    bounds: Option<(i64, i64)>,
    frames: usize,
    events: usize,
    chase: usize,
}

/// Lookups a prepared plan reads, shared while a track's notes are unchanged.
#[derive(Clone, Debug)]
pub(crate) struct TrackDerived {
    /// The cursor's note lookups for this track. Sharing it tells the cursor
    /// the notes are unchanged, so it takes over that track's voices (R1).
    pub(crate) index: Arc<NoteIndex>,
}

impl TrackDerived {
    fn new(plan: &TrackPlan) -> Self {
        Self {
            index: Arc::new(NoteIndex::new(plan)),
        }
    }
}

/// A complete plan and the lookups derived from each of its tracks, in order.
pub(crate) struct Built {
    pub(crate) plan: Plan,
    pub(crate) derived: Vec<TrackDerived>,
}

impl PlaybackIds {
    pub fn new() -> Self {
        Self::default()
    }

    fn track(&mut self, id: u128) -> Result<u32, PlaybackError> {
        assign(&mut self.tracks, &mut self.next_track, id)
    }

    /// The current number of `model`, or a new one held in `fresh` until the
    /// build succeeds. A failed build discards `fresh`; its numbers stay used.
    fn note(&mut self, fresh: &mut HashMap<u128, u32>, model: u128) -> Result<u32, PlaybackError> {
        if let Some(number) = self.notes.get(&model) {
            return Ok(number.id);
        }
        assign(fresh, &mut self.next_note, model)
    }

    /// `(events, chase)` when the track's current plan was sized for these
    /// loop bounds and block frames.
    pub(crate) fn demand(
        &self,
        track: u128,
        bounds: Option<(i64, i64)>,
        frames: usize,
    ) -> Option<(usize, usize)> {
        self.planned
            .get(&track)
            .and_then(|entry| entry.demand)
            .filter(|demand| demand.bounds == bounds && demand.frames == frames)
            .map(|demand| (demand.events, demand.chase))
    }

    pub(crate) fn remember_demand(
        &mut self,
        track: &TrackPlan,
        bounds: Option<(i64, i64)>,
        frames: usize,
        events: usize,
        chase: usize,
    ) {
        #[cfg(test)]
        {
            self.derived_notes += track.notes.len();
        }
        if let Some(entry) = self.planned.get_mut(&track.track) {
            entry.demand = Some(CachedDemand {
                bounds,
                frames,
                events,
                chase,
            });
        }
    }

    /// Publishes a complete build. Spans of rebuilt plans are counted before
    /// the plans they replace are released, so a note that moved keeps its number.
    fn commit(
        &mut self,
        planned: HashMap<u128, PlannedTrack>,
        tracks: HashMap<u128, u32>,
        basis: (TempoMap, u32),
    ) {
        for entry in planned.values().filter(|entry| entry.rebuilt) {
            for note in &entry.plan.notes {
                self.notes
                    .entry(note.note)
                    .and_modify(|number| number.uses += 1)
                    .or_insert(NoteNumber {
                        id: note.note_id,
                        uses: 1,
                    });
            }
        }
        for (track, previous) in &self.planned {
            if planned.get(track).is_some_and(|entry| !entry.rebuilt) {
                continue;
            }
            for note in &previous.plan.notes {
                if let Some(number) = self.notes.get_mut(&note.note) {
                    number.uses -= 1;
                    if number.uses == 0 {
                        self.notes.remove(&note.note);
                    }
                }
            }
        }
        // Release capacity left by large, since-deleted scores.
        if self.notes.len().saturating_mul(4) < self.notes.capacity() {
            self.notes.shrink_to_fit();
        }
        self.planned = planned;
        self.tracks = tracks;
        self.basis = Some(basis);
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
    build(project, tempo, sample_rate, ids).map(|built| built.plan)
}

/// Builds the plan, reusing each track whose inputs are unchanged.
pub(crate) fn build(
    project: &Project,
    tempo: &TempoMap,
    sample_rate: u32,
    ids: &mut PlaybackIds,
) -> Result<Built, PlaybackError> {
    let index = tempo_index(tempo)?;
    #[cfg(test)]
    {
        ids.planned_notes = 0;
        ids.derived_notes = 0;
    }
    let reusable = ids
        .basis
        .as_ref()
        .is_some_and(|(map, rate)| map == tempo && *rate == sample_rate);
    let count = project.tracks().len();
    let mut tracks = Vec::with_capacity(count);
    let mut derived = Vec::with_capacity(count);
    let mut current_tracks = HashMap::with_capacity(count);
    let mut planned = HashMap::with_capacity(count);
    let mut fresh = HashMap::new();
    for track in project.tracks() {
        let track_key = ids.track(track.id.raw())?;
        current_tracks.insert(track.id.raw(), track_key);
        let contents = track
            .clips
            .iter()
            .map(|clip| project.shared_content(clip.content).cloned())
            .collect::<Option<Vec<_>>>()
            .ok_or(PlaybackError::Time)?;
        if reusable
            && let Some(previous) = ids.planned.get(&track.id.raw())
            && Arc::ptr_eq(&previous.track, track)
            && previous.contents.len() == contents.len()
            && previous
                .contents
                .iter()
                .zip(&contents)
                .all(|(before, now)| Arc::ptr_eq(before, now))
        {
            // A surviving track keeps its key, so the stored plan still names it.
            tracks.push(TrackPlan::clone(&previous.plan));
            derived.push(previous.derived.clone());
            planned.insert(
                track.id.raw(),
                PlannedTrack {
                    track: Arc::clone(track),
                    contents,
                    plan: Arc::clone(&previous.plan),
                    derived: previous.derived.clone(),
                    demand: previous.demand,
                    rebuilt: false,
                },
            );
            continue;
        }
        let mut notes = Vec::new();
        for (clip, content) in track.clips.iter().zip(&contents) {
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
                let note_id = ids.note(&mut fresh, note.id.raw())?;
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
        #[cfg(test)]
        {
            ids.planned_notes += notes.len();
        }
        notes.sort_by(|left, right| {
            left.on
                .cmp(&right.on)
                .then(left.pitch.cmp(&right.pitch))
                .then(left.note.cmp(&right.note))
        });
        let mut by_off: Vec<u32> = (0..notes.len()).map(|index| index as u32).collect();
        by_off.sort_by_key(|index| {
            notes
                .get(*index as usize)
                .map(|note| (note.off, note.pitch, note.note))
        });
        let mixer = mixer_values(track)?;
        let plan = TrackPlan {
            track: track.id.raw(),
            track_key,
            notes,
            by_off,
            gain: mixer.0,
            pan: mixer.1,
            mute: mixer.2,
            solo: mixer.3,
        };
        let lookups = TrackDerived::new(&plan);
        #[cfg(test)]
        {
            ids.derived_notes += plan.notes.len();
        }
        tracks.push(plan.clone());
        derived.push(lookups.clone());
        planned.insert(
            track.id.raw(),
            PlannedTrack {
                track: Arc::clone(track),
                contents,
                plan: Arc::new(plan),
                derived: lookups,
                demand: None,
                rebuilt: true,
            },
        );
    }
    // Publish only after the complete build succeeds. Dropping superseded hash
    // storage here also releases capacity left by large, since-deleted scores.
    ids.commit(planned, current_tracks, (tempo.clone(), sample_rate));
    Ok(Built {
        plan: Plan { tracks },
        derived,
    })
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

#[cfg(test)]
#[path = "tempo_tests.rs"]
mod tempo_tests;
