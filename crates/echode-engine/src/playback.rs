//! Product playback. All score, instrument, and scratch storage is prepared by
//! the control thread. The callback only moves prepared ownership through rings.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use crate::TrackMeter;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use echode_model::{Meter, Project};
use echode_realtime::{Cursor, Event, Plan, Span, TempoIndex, sort_events};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::mix::{FaderState, Signal};
use crate::plugins::{ChainDevice, HostGraph, PluginRoute, TrackChain, collect_deadline};
use crate::realtime::ForbidAlloc;
use crate::sequence::render_metronome_index_before;
use crate::{Device, PlaybackError, PlaybackIds, PlayedSpan, TestSynth, TrackFader};

/// Clock providers consume device frames. External synchronization may supply
/// another provider without making the UI timer advance the transport.
pub trait ClockSource: Send {
    fn position(&self) -> i64;
    fn locate(&mut self, sample: i64);
    fn advance(&mut self, frames: u32) -> Result<(), PlaybackError>;
}

#[derive(Clone, Debug, Default)]
pub struct InternalClock {
    position: i64,
}

impl ClockSource for InternalClock {
    fn position(&self) -> i64 {
        self.position
    }
    fn locate(&mut self, sample: i64) {
        self.position = sample;
    }
    fn advance(&mut self, frames: u32) -> Result<(), PlaybackError> {
        self.position = self
            .position
            .checked_add(i64::from(frames))
            .ok_or(PlaybackError::Time)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PlaybackCommand {
    Play,
    Pause,
    Stop,
    Locate(i64),
    /// Loop bounds in device samples. `end` is exclusive and must be after `start`.
    SetLoop {
        start: i64,
        end: i64,
    },
    ClearLoop,
    SetMetronome(bool),
    /// Transient UI audition. The durable mixer stays in the project plan.
    PreviewFader {
        track: u128,
        gain: f32,
        pan: f32,
    },
    ClearFaderPreview {
        after_plan: u64,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub sample: i64,
    pub tick: i64,
    pub failed: bool,
    pub applied_command: u64,
    /// Cumulative callbacks with a plugin processing/submit/deadline failure.
    pub plugin_missed_blocks: u64,
    /// Compatibility counter; exclusive audio ownership keeps this at zero.
    pub graph_unavailable_blocks: u64,
    /// Control has accepted a score whose prepared bank is not yet audible.
    pub pending_plan: bool,
}

struct Status {
    generation: AtomicU32,
    playing: AtomicBool,
    sample: AtomicI64,
    tick: AtomicI64,
    failed: AtomicBool,
    applied_command: AtomicU64,
    plugin_missed_blocks: AtomicU64,
    graph_unavailable_blocks: AtomicU64,
    requested_plan: AtomicU64,
    applied_plan: AtomicU64,
}

struct Track {
    key: u32,
    synth: TestSynth,
    plane: Vec<f32>,
    right: Vec<f32>,
    /// Placed audio for this block, kept apart from the synth so an instrument
    /// can replace the synth without eating the clips.
    clip_plane: Vec<f32>,
    clip_right: Vec<f32>,
    clip_stereo: bool,
    stereo: bool,
    fader: FaderState,
    base_fader: TrackFader,
    meter: Arc<TrackMeter>,
    cache: Option<CachedTrack>,
    /// Playback ids sorted for bounded lookup when carrying a live voice.
    notes: Vec<(u32, i64, i64)>,
    clips: Vec<StoredClip>,
}

struct StoredClip {
    start_sample: i64,
    samples: Arc<[f32]>,
    right: Option<Arc<[f32]>>,
    fade_in: u32,
    fade_out: u32,
}

/// One placement's samples at the device rate, ready for the control thread to store.
#[derive(Clone, Debug)]
pub struct LoadedClip {
    pub track: u128,
    pub start_sample: i64,
    pub samples: Arc<[f32]>,
    /// The right plane of a stereo placement. `samples` is its left plane.
    /// `None` describes mono. Both stereo planes must have the same length.
    pub right: Option<Arc<[f32]>>,
    pub fade_in: u32,
    pub fade_out: u32,
}

/// A ready device-output plane, before the track fader. The provider owns
/// rendering/provenance/invalidation; the engine only selects and reads it.
#[derive(Clone, Debug)]
pub struct CachedTrack {
    pub track: u128,
    pub sample_rate: u32,
    pub start_sample: i64,
    pub samples: Arc<[f32]>,
    pub right: Option<Arc<[f32]>>,
}

struct PreparedSources<'a> {
    clips: &'a [LoadedClip],
    caches: &'a BTreeMap<u128, CachedTrack>,
}

/// Loads device-rate clips while a plan is built. The audio thread never calls it.
pub type ClipLoader =
    Arc<dyn Fn(&Project, u32) -> Result<Vec<LoadedClip>, PlaybackError> + Send + Sync>;

fn empty_clips(_project: &Project, _rate: u32) -> Result<Vec<LoadedClip>, PlaybackError> {
    Ok(Vec::new())
}

struct Prepared {
    plan: Plan,
    cursor: Cursor,
    tempo: TempoIndex,
    tracks: Vec<Track>,
    track_indices: Vec<(u32, usize)>,
    mixer_indices: Vec<(u128, usize)>,
    events: Vec<Event>,
    filtered: Vec<Event>,
    /// Index workspace for `sort_events`. Same length as `events`.
    sort_order: Vec<usize>,
    /// Note-offs already written at the front of `events`, not yet submitted.
    released: usize,
    sample_rate: u32,
    meter: Meter,
    chase: bool,
    /// The plugin has been told about the cursor's sounding notes.
    plugin_holds: bool,
    /// Note-ons chased from the previous plan. The synth already has these voices.
    /// A plugin that was just spawned still needs them. Timeline note-ons stay in `events`.
    chased_ons: Vec<Event>,
    /// Installed with the event buffers. The callback does not change these alone.
    loop_start: Option<i64>,
    loop_end: Option<i64>,
    demands: Vec<TrackEventDemand>,
    mix_left: Vec<f64>,
    mix_right: Vec<f64>,
}

#[derive(Clone, Copy)]
struct RenderRange {
    loop_bounds: (Option<i64>, Option<i64>),
    started_before: Option<i64>,
}

#[derive(Clone, Copy)]
struct StopTail {
    position: i64,
    cutoff: i64,
    remaining: u32,
    loop_bounds: (Option<i64>, Option<i64>),
}

impl Prepared {
    fn new(
        project: &Project,
        rate: u32,
        frames: usize,
        ids: &mut PlaybackIds,
        sources: PreparedSources<'_>,
        loop_bounds: Option<(i64, i64)>,
        meters: &mut BTreeMap<u128, Arc<TrackMeter>>,
    ) -> Result<Self, PlaybackError> {
        if rate == 0 || frames == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let loaded = sources.clips;
        if loaded.iter().any(|clip| {
            clip.right
                .as_ref()
                .is_some_and(|right| right.len() != clip.samples.len())
                || !project
                    .tracks()
                    .iter()
                    .any(|track| track.id.raw() == clip.track)
        }) {
            return Err(PlaybackError::Media);
        }
        let plan = crate::plan::build_plan(project, project.tempo(), rate, ids)?;
        let cursor = plan.cursor();
        let tempo = crate::plan::tempo_index(project.tempo())?;
        let solo = plan.tracks.iter().any(|track| track.solo != 0.0);
        let mut tracks = Vec::with_capacity(plan.tracks.len());
        let iterations = loop_iterations(frames, loop_bounds)?;
        let mut event_count = 1usize;
        let mut chase_count = 0usize;
        let mut demands = Vec::with_capacity(plan.tracks.len());
        for track in &plan.tracks {
            let mut synth = TestSynth::new(rate)?;
            let demand =
                crate::plan::voice_demand(&track.notes, i64::from(synth.tail_samples(rate)));
            synth.prepare(demand.max(1), rate)?;
            let mut notes: Vec<_> = track
                .notes
                .iter()
                .map(|note| (note.note_id, note.on, note.off))
                .collect();
            notes.sort_unstable_by_key(|note| note.0);
            let mut clips: Vec<StoredClip> = loaded
                .iter()
                .filter(|clip| clip.track == track.track)
                .map(|clip| StoredClip {
                    start_sample: clip.start_sample,
                    samples: Arc::clone(&clip.samples),
                    right: clip.right.clone(),
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                })
                .collect();
            clips.sort_by_key(|clip| clip.start_sample);
            let clip_stereo = clips.iter().any(|clip| clip.right.is_some());
            if clip_stereo {
                for clip in &mut clips {
                    if clip.right.is_none() {
                        clip.samples = clip
                            .samples
                            .iter()
                            .map(|sample| sample * std::f32::consts::FRAC_1_SQRT_2)
                            .collect();
                        clip.right = Some(Arc::clone(&clip.samples));
                    }
                }
            }
            let voices = demand;
            let demand = track_event_demand(track, loop_bounds, iterations, voices)?;
            event_count = event_count
                .checked_add(demand.events)
                .ok_or(PlaybackError::Time)?;
            chase_count = chase_count
                .checked_add(demand.chase)
                .ok_or(PlaybackError::Time)?;
            demands.push(demand);
            let fader = TrackFader {
                gain: track.gain,
                pan: track.pan,
                mute: track.mute != 0.0 || (solo && track.solo == 0.0),
                solo: false,
            };
            tracks.push(Track {
                key: track.track_key,
                meter: Arc::clone(
                    meters
                        .entry(track.track)
                        .or_insert_with(|| Arc::new(TrackMeter::new(track.track))),
                ),
                synth,
                plane: vec![0.0; frames],
                right: vec![0.0; frames],
                clip_plane: vec![0.0; frames],
                clip_right: vec![0.0; frames],
                clip_stereo,
                stereo: clip_stereo,
                fader: FaderState::new(fader, rate),
                base_fader: fader,
                cache: sources.caches.get(&track.track).cloned(),
                notes,
                clips,
            });
        }
        event_count = event_count
            .checked_add(chase_count)
            .ok_or(PlaybackError::Time)?
            .max(1);
        let mut track_indices: Vec<_> = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.key, index))
            .collect();
        track_indices.sort_unstable_by_key(|entry| entry.0);
        let mut mixer_indices: Vec<_> = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.meter.track, index))
            .collect();
        mixer_indices.sort_unstable_by_key(|entry| entry.0);
        Ok(Self {
            plan,
            cursor,
            tempo,
            tracks,
            track_indices,
            mixer_indices,
            events: vec![Event::note_off(0, 0, 0, 0); event_count],
            filtered: vec![Event::note_off(0, 0, 0, 0); event_count],
            sort_order: vec![0; event_count],
            released: 0,
            sample_rate: rate,
            meter: project.meter(),
            chase: true,
            plugin_holds: false,
            chased_ons: Vec::with_capacity(chase_count.max(1)),
            loop_start: loop_bounds.map(|(start, _)| start),
            loop_end: loop_bounds.map(|(_, end)| end),
            demands,
            mix_left: vec![0.0; frames],
            mix_right: vec![0.0; frames],
        })
    }

    fn mixer_slot(&self, id: u128) -> Option<usize> {
        self.mixer_indices
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()
            .and_then(|index| self.mixer_indices.get(index))
            .map(|entry| entry.1)
    }

    fn preview_fader(
        &mut self,
        previous: Option<u128>,
        preview: Option<(u128, f32, f32)>,
        playing: bool,
    ) {
        let apply = |track: &mut Track, values: Option<(f32, f32)>| {
            let mut target = track.base_fader;
            if let Some((gain, pan)) = values {
                target.gain = gain;
                target.pan = pan;
            }
            track.fader.retarget(target);
            if !playing {
                track.fader.settle();
            }
        };
        // Resolve only the old and new audition targets through an index built
        // on the control thread; dragging never scans the entire track list.
        if previous != preview.map(|entry| entry.0)
            && let Some(slot) = previous.and_then(|id| self.mixer_slot(id))
            && let Some(track) = self.tracks.get_mut(slot)
        {
            apply(track, None);
        }
        if let Some((id, gain, pan)) = preview
            && let Some(slot) = self.mixer_slot(id)
            && let Some(track) = self.tracks.get_mut(slot)
        {
            apply(track, Some((gain, pan)));
        }
    }

    fn place(&mut self, sample: i64, silence: bool) -> Result<(), PlaybackError> {
        if silence && self.plugin_holds {
            self.queue_all_offs()?;
            self.plugin_holds = false;
        }
        self.cursor.catch_up(&self.plan, sample);
        self.chase = true;
        if silence {
            for track in &mut self.tracks {
                track.synth.silence();
            }
        }
        Ok(())
    }

    /// Note-offs for every note this cursor has already sent to the plugin.
    fn queue_all_offs(&mut self) -> Result<(), PlaybackError> {
        let sample = i64::MAX / 4;
        let start = self.released;
        let Prepared {
            events,
            cursor,
            plan,
            released,
            ..
        } = self;
        let rest = events
            .get_mut(start..)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let count = cursor
            .release_replaced(plan, plan, sample, 0, rest)
            .map_err(play_error)?;
        *released = start + count;
        Ok(())
    }

    /// Note-offs for notes the previous plan was holding that `self` does not cover.
    fn queue_replaced(&mut self, previous: &Self, sample: i64) -> Result<(), PlaybackError> {
        let start = self.released;
        let Prepared {
            events,
            plan,
            released,
            ..
        } = self;
        let rest = events
            .get_mut(start..)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let count = previous
            .cursor
            .release_replaced(&previous.plan, plan, sample, 0, rest)
            .map_err(play_error)?;
        *released = start + count;
        Ok(())
    }

    fn carry(
        &mut self,
        previous: &mut Self,
        sample: i64,
        playing: bool,
    ) -> Result<(), PlaybackError> {
        for track in &mut self.tracks {
            let old_index = previous
                .track_indices
                .binary_search_by_key(&track.key, |entry| entry.0)
                .ok()
                .and_then(|index| previous.track_indices.get(index))
                .map(|entry| entry.1);
            if let Some(old) = old_index.and_then(|index| previous.tracks.get_mut(index)) {
                if playing {
                    track.fader.carry(old.fader);
                }
                let notes = &track.notes;
                track.synth.carry_from(&mut old.synth, |id| {
                    let note = notes
                        .binary_search_by_key(&id, |note| note.0)
                        .ok()
                        .and_then(|index| notes.get(index))?;
                    let tail = i64::from(self.sample_rate) * 400 / 1000;
                    (note.1 < sample && note.2.saturating_add(tail) > sample)
                        .then_some(note.2 >= sample)
                });
            }
        }
        if previous.plugin_holds {
            self.queue_replaced(previous, sample)?;
        }
        self.place(sample, false)?;
        self.cursor
            .keep_live_keys(&self.plan, &previous.cursor, &previous.plan);
        self.plugin_holds = previous.plugin_holds;
        Ok(())
    }

    /// Moves chased note-ons whose voice was carried into `chased_ons`.
    ///
    /// Playing them again would restart the built-in envelope. They stay available
    /// for a plugin that has not heard them yet. Note-ons `play` adds afterwards,
    /// including a loop restart, remain in `events`.
    fn take_carried_note_ons(&mut self, count: usize) -> Result<usize, PlaybackError> {
        self.chased_ons.clear();
        let mut kept = 0usize;
        for index in 0..count {
            let event = *self
                .events
                .get(index)
                .ok_or(PlaybackError::BufferTooSmall)?;
            if event.is_note_on() && self.voice_is_sounding(event) {
                if self.chased_ons.len() == self.chased_ons.capacity() {
                    return Err(PlaybackError::BufferTooSmall);
                }
                self.chased_ons.push(event);
                continue;
            }
            *self
                .events
                .get_mut(kept)
                .ok_or(PlaybackError::BufferTooSmall)? = event;
            kept += 1;
        }
        Ok(kept)
    }

    fn voice_is_sounding(&self, event: Event) -> bool {
        let Some(id) = event.note_id() else {
            return false;
        };
        let Some(key) = event.track_key() else {
            return false;
        };
        self.track_indices
            .binary_search_by_key(&key, |entry| entry.0)
            .ok()
            .and_then(|index| self.track_indices.get(index))
            .and_then(|entry| self.tracks.get(entry.1))
            .is_some_and(|track| track.synth.sounding(id))
    }

    fn render(
        &mut self,
        spans: &[Span],
        left: &mut [f32],
        right: &mut [f32],
        hosts: &mut HostGraph,
        deadline: Instant,
        range: RenderRange,
    ) -> Result<(), PlaybackError> {
        hosts.begin_block();
        let count = if range.started_before.is_some() {
            0
        } else {
            let mut count = self.released;
            self.released = 0;
            if self.chase {
                self.chase = false;
                let rest = self
                    .events
                    .get_mut(count..)
                    .ok_or(PlaybackError::BufferTooSmall)?;
                count += self
                    .cursor
                    .chase_events(&self.plan, rest)
                    .map_err(play_error)?;
            }
            count = self.take_carried_note_ons(count)?;
            let rest = self
                .events
                .get_mut(count..)
                .ok_or(PlaybackError::BufferTooSmall)?;
            count += self
                .cursor
                .play(
                    &self.plan,
                    spans,
                    rest,
                    &mut self.sort_order,
                    &mut self.filtered,
                )
                .map_err(play_error)?;
            if !sort_events(
                self.events
                    .get_mut(..count)
                    .ok_or(PlaybackError::BufferTooSmall)?,
                &mut self.sort_order,
                &mut self.filtered,
            ) {
                return Err(PlaybackError::BufferTooSmall);
            }
            count
        };
        let events = self
            .events
            .get(..count)
            .ok_or(PlaybackError::BufferTooSmall)?;
        for track in &mut self.tracks {
            let frames = left.len();
            let Track {
                key,
                synth,
                plane,
                right,
                clip_plane,
                clip_right,
                clip_stereo,
                stereo,
                clips,
                cache,
                ..
            } = track;
            let plane = plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let clip_plane = clip_plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let right = right
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let clip_right = clip_right
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            plane.fill(0.0);
            right.fill(0.0);
            clip_plane.fill(0.0);
            clip_right.fill(0.0);
            if let Some(cache) = cache {
                read_cached_track(cache, spans, plane, right)?;
                *stereo = cache.right.is_some();
                continue;
            }
            let mut kept = 0usize;
            for event in events.iter() {
                if event.track_key() != Some(*key) {
                    continue;
                }
                let slot = self
                    .filtered
                    .get_mut(kept)
                    .ok_or(PlaybackError::BufferTooSmall)?;
                *slot = *event;
                kept += 1;
            }
            let filtered = self
                .filtered
                .get(..kept)
                .ok_or(PlaybackError::BufferTooSmall)?;
            synth.process(&mut [plane], filtered, self.sample_rate)?;
            add_stored_clips(
                clips,
                spans,
                clip_plane,
                clip_right,
                *clip_stereo,
                range,
                self.sample_rate,
            )?;
            *stereo = *clip_stereo;
            for (((sample, right), clip), clip_right) in plane
                .iter_mut()
                .zip(right.iter_mut())
                .zip(clip_plane.iter())
                .zip(clip_right.iter())
            {
                if *clip_stereo {
                    *sample *= std::f32::consts::FRAC_1_SQRT_2;
                    *right = *sample + *clip_right;
                }
                *sample += *clip;
            }
            hosts.submit(
                *key,
                Signal {
                    left: plane,
                    right: stereo.then_some(&*right),
                },
                Signal {
                    left: clip_plane,
                    right: clip_stereo.then_some(&*clip_right),
                },
                filtered,
                &self.chased_ons,
            );
        }
        if hosts.dropped_events() {
            return Err(PlaybackError::BufferTooSmall);
        }
        hosts.collect(collect_deadline(deadline, left.len(), self.sample_rate));
        let mix_left = self
            .mix_left
            .get_mut(..left.len())
            .ok_or(PlaybackError::BufferTooSmall)?;
        let mix_right = self
            .mix_right
            .get_mut(..right.len())
            .ok_or(PlaybackError::BufferTooSmall)?;
        mix_left.fill(0.0);
        mix_right.fill(0.0);
        for track in &mut self.tracks {
            let plane = track
                .plane
                .get_mut(..left.len())
                .ok_or(PlaybackError::BufferTooSmall)?;
            let track_right = track
                .right
                .get_mut(..right.len())
                .ok_or(PlaybackError::BufferTooSmall)?;
            if track.cache.is_none()
                && let Some(stereo) = hosts.apply(track.key, plane, track_right)
            {
                track.stereo = stereo;
            }
            let peak = track.fader.add(
                Signal {
                    left: plane,
                    right: track.stereo.then_some(&*track_right),
                },
                mix_left,
                mix_right,
            );
            track.meter.publish(peak, 1.0);
        }
        for ((left, right), (l, r)) in left
            .iter_mut()
            .zip(right)
            .zip(mix_left.iter().zip(mix_right.iter()))
        {
            *left = *l as f32;
            *right = *r as f32;
        }
        self.plugin_holds = true;
        Ok(())
    }

    /// Deliver note-offs through one discarded plugin block. This does not
    /// drain a release or delay tail; discontinuities separately request a DSP
    /// reset before the helper receives any new block's events.
    fn flush_releases(
        &mut self,
        hosts: &mut HostGraph,
        deadline: Instant,
    ) -> Result<(), PlaybackError> {
        if self.released == 0 {
            return Ok(());
        }
        hosts.begin_block();
        let count = self.released;
        self.released = 0;
        let frames = self
            .tracks
            .first()
            .map(|track| track.plane.len())
            .unwrap_or(0);
        let events = self
            .events
            .get(..count)
            .ok_or(PlaybackError::BufferTooSmall)?;
        for track in &mut self.tracks {
            let plane = track
                .plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            plane.fill(0.0);
            let mut kept = 0usize;
            for event in events.iter() {
                if event.track_key() != Some(track.key) {
                    continue;
                }
                let slot = self
                    .filtered
                    .get_mut(kept)
                    .ok_or(PlaybackError::BufferTooSmall)?;
                *slot = *event;
                kept += 1;
            }
            let filtered = self
                .filtered
                .get(..kept)
                .ok_or(PlaybackError::BufferTooSmall)?;
            hosts.submit(
                track.key,
                Signal {
                    left: plane,
                    right: None,
                },
                Signal {
                    left: &[],
                    right: None,
                },
                filtered,
                &[],
            );
        }
        hosts.collect(collect_deadline(deadline, frames, self.sample_rate));
        Ok(())
    }

    fn queue_cleanup(&mut self, hosts: &mut HostGraph) -> Result<(), PlaybackError> {
        let events = self
            .events
            .get(..self.released)
            .ok_or(PlaybackError::BufferTooSmall)?;
        if !hosts.queue_cleanup(events) {
            return Err(PlaybackError::BufferTooSmall);
        }
        self.released = 0;
        Ok(())
    }
}

fn read_cached_track(
    cache: &CachedTrack,
    spans: &[Span],
    left: &mut [f32],
    right: &mut [f32],
) -> Result<(), PlaybackError> {
    for span in spans {
        let played = [PlayedSpan {
            start_sample: span.start,
            frames: span.frames,
            block_offset: span.block_offset,
            release_at: None,
            loop_end: None,
            wrap_from: None,
        }];
        crate::voice::render_track(
            crate::voice::TrackSource::Cache {
                samples: &cache.samples,
                origin: cache.start_sample,
            },
            &[],
            &played,
            left,
        )?;
        if let Some(samples) = &cache.right {
            crate::voice::render_track(
                crate::voice::TrackSource::Cache {
                    samples,
                    origin: cache.start_sample,
                },
                &[],
                &played,
                right,
            )?;
        }
    }
    Ok(())
}

fn add_stored_clips(
    clips: &[StoredClip],
    spans: &[Span],
    output: &mut [f32],
    right: &mut [f32],
    stereo: bool,
    range: RenderRange,
    rate: u32,
) -> Result<(), PlaybackError> {
    if clips.is_empty() {
        return Ok(());
    }
    // Sorted by start. Placements do not overlap, so ends follow the same order.
    // The callback binary-searches the first clip that can reach the span and
    // stops at the first one that starts after it.
    for span in spans {
        let mut end = span
            .start
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        if range.started_before.is_some()
            && let Some(loop_end) = range.loop_bounds.1
        {
            // A stop continues the current placement, never the next loop or
            // audio beyond its boundary. Its existing seam fade still applies.
            end = end.min(loop_end);
        }
        if end <= span.start {
            continue;
        }
        let clip_frames = u32::try_from(end - span.start).map_err(|_| PlaybackError::Time)?;
        let first = clips.partition_point(|clip| stored_end(clip) <= span.start);
        let Some(rest) = clips.get(first..) else {
            continue;
        };
        for clip in rest.iter().take_while(|clip| clip.start_sample < end) {
            if range
                .started_before
                .is_some_and(|cutoff| clip.start_sample >= cutoff)
            {
                continue;
            }
            let mut view = [crate::voice::FadedClip {
                start_sample: clip.start_sample,
                samples: clip.samples.as_ref(),
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
            }];
            let played = [PlayedSpan {
                start_sample: span.start,
                frames: clip_frames,
                block_offset: span.block_offset,
                release_at: None,
                loop_end: None,
                wrap_from: None,
            }];
            crate::voice::render_faded_clips(&view, &played, output)?;
            if stereo {
                view[0].samples = clip.right.as_deref().unwrap_or(&clip.samples);
                crate::voice::render_faded_clips(&view, &played, right)?;
            }
        }
        let played = [PlayedSpan {
            start_sample: span.start,
            frames: span.frames,
            block_offset: span.block_offset,
            release_at: None,
            loop_end: range.loop_bounds.1,
            wrap_from: range.loop_bounds.0,
        }];
        if range.loop_bounds.0.is_some() && range.loop_bounds.1.is_some() {
            // Keep a nonzero interior in short loops. One/two-frame loops
            // cannot fit two endpoints and an interior, so leave them intact.
            let fade = range
                .loop_bounds
                .1
                .zip(range.loop_bounds.0)
                .and_then(|(end, start)| end.checked_sub(start))
                .filter(|length| *length > 2)
                .map_or(0, |length| {
                    crate::voice::seam_fade_frames(rate)
                        .min(u32::try_from(length / 2).unwrap_or(u32::MAX))
                });
            crate::voice::fade_loop_seam(&played, fade, output)?;
            if stereo {
                crate::voice::fade_loop_seam(&played, fade, right)?;
            }
        }
    }
    Ok(())
}

fn stored_end(clip: &StoredClip) -> i64 {
    clip.start_sample
        .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX))
}

struct TrackEventDemand {
    key: u32,
    events: usize,
    chase: usize,
}

fn loop_iterations(frames: usize, bounds: Option<(i64, i64)>) -> Result<usize, PlaybackError> {
    let Some((start, end)) = bounds else {
        return Ok(1);
    };
    if end <= start {
        return Ok(1);
    }
    let len = usize::try_from(end - start).map_err(|_| PlaybackError::Time)?;
    if len == 0 {
        return Err(PlaybackError::InvalidLoop);
    }
    frames
        .checked_add(len - 1)
        .and_then(|sum| sum.checked_div(len))
        .filter(|count| *count > 0)
        .ok_or(PlaybackError::Time)
}

fn track_event_demand(
    track: &echode_realtime::TrackPlan,
    bounds: Option<(i64, i64)>,
    iterations: usize,
    voices: usize,
) -> Result<TrackEventDemand, PlaybackError> {
    let mut events = 0usize;
    for note in &track.notes {
        let inside = bounds.is_some_and(|(start, end)| note.on < end && note.off > start);
        let times = if inside { iterations } else { 1 };
        events = events
            .checked_add(3)
            .and_then(|count| count.checked_mul(times))
            .ok_or(PlaybackError::Time)?;
    }
    let chase = voices.max(1);
    Ok(TrackEventDemand {
        key: track.track_key,
        events,
        chase,
    })
}

fn chain_of(project: &Project, prepared: &Prepared) -> Vec<TrackChain> {
    prepared
        .plan
        .tracks
        .iter()
        .filter_map(|planned| {
            if prepared
                .mixer_slot(planned.track)
                .and_then(|slot| prepared.tracks.get(slot))
                .is_some_and(|track| track.cache.is_some())
            {
                return None;
            }
            let track = project
                .tracks()
                .iter()
                .find(|track| track.id.raw() == planned.track)?;
            let devices = track
                .devices
                .iter()
                .filter_map(|device| {
                    Some(ChainDevice {
                        id: device.id.raw(),
                        clap_id: device.plugin.clone()?,
                    })
                })
                .collect();
            let demand = prepared
                .demands
                .iter()
                .find(|demand| demand.key == planned.track_key);
            Some(TrackChain {
                key: planned.track_key,
                stereo: prepared
                    .tracks
                    .iter()
                    .find(|track| track.key == planned.track_key)
                    .is_some_and(|track| track.clip_stereo),
                devices,
                events: demand.map(|demand| demand.events).unwrap_or(1),
                chase: demand.map(|demand| demand.chase).unwrap_or(1),
            })
        })
        .collect()
}

fn play_error(error: echode_realtime::PlayError) -> PlaybackError {
    match error {
        echode_realtime::PlayError::Time => PlaybackError::Time,
        echode_realtime::PlayError::BufferTooSmall => PlaybackError::BufferTooSmall,
    }
}

/// A score and its compatible audio bank are published as one ownership unit.
struct Update {
    prepared: Box<Prepared>,
    hosts: Box<HostGraph>,
    revision: u64,
}

struct Pending {
    prepared: Box<Prepared>,
    chain: Vec<TrackChain>,
    routes: Vec<PluginRoute>,
    revision: u64,
}

pub struct PlaybackControl {
    plans: Producer<Update>,
    returns: Consumer<Update>,
    commands: Producer<(u64, PlaybackCommand)>,
    command_revision: u64,
    held: Option<Pending>,
    hosts: HostGraph,
    preparation_error: Option<PlaybackError>,
    published_plan: u64,
    chain: Vec<TrackChain>,
    routes: Vec<PluginRoute>,
    host_bin: std::path::PathBuf,
    ids: PlaybackIds,
    meters: BTreeMap<u128, Arc<TrackMeter>>,
    caches: BTreeMap<u128, CachedTrack>,
    rate: u32,
    frames: usize,
    status: Arc<Status>,
    clip_loader: ClipLoader,
    project: Project,
    loop_bounds: Option<(i64, i64)>,
    /// Event slots in the plan most recently prepared. Not a high-water mark.
    reserved_events: usize,
    /// Plugin bytes saved on the control thread and not yet given to the document.
    plugin_saves: Vec<(u128, Vec<u8>)>,
    save_controls: Vec<(u128, echode_sandbox::PluginControl)>,
}

/// Owned by exactly one callback. Move back to the control thread before Drop.
pub struct PlaybackAudio {
    plans: Consumer<Update>,
    returns: Producer<Update>,
    commands: Consumer<(u64, PlaybackCommand)>,
    applied_command: u64,
    reset_plugins: bool,
    output_revision: u64,
    live_output: bool,
    stop_tail: Option<StopTail>,
    stop_fade_offset: Option<u32>,
    fader_preview: Option<(u128, f32, f32)>,
    clear_preview_after: Option<u64>,
    hosts: Box<HostGraph>,
    current: Box<Prepared>,
    clock: Box<dyn ClockSource>,
    playing: bool,
    failed: bool,
    loop_start: Option<i64>,
    loop_end: Option<i64>,
    metronome: bool,
    /// One slot per prepared frame. A one-sample loop can fill every slot.
    spans: Vec<Span>,
    played: Vec<PlayedSpan>,
    clicks: Vec<f32>,
    status: Arc<Status>,
}

pub fn playback_pair(
    project: &Project,
    rate: u32,
    frames: usize,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    playback_with_clock(project, rate, frames, Box::<InternalClock>::default())
}

/// Prepare both the clock and published position before a device can call back.
pub fn playback_pair_at(
    project: &Project,
    rate: u32,
    frames: usize,
    sample: i64,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    if sample < 0 {
        return Err(PlaybackError::Time);
    }
    playback_with_clock(
        project,
        rate,
        frames,
        Box::new(InternalClock { position: sample }),
    )
}

/// Same transport as [`playback_pair`], and a helper for each resolved CLAP
/// device. Devices whose id is not in `routes` stay on the built-in synth.
pub fn playback_with_plugins(
    project: &Project,
    rate: u32,
    frames: usize,
    host_bin: &std::path::Path,
    routes: &[PluginRoute],
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    playback_with_plugins_at(project, rate, frames, host_bin, routes, 0)
}

/// Hosted playback with an initial position already visible to status readers.
pub fn playback_with_plugins_at(
    project: &Project,
    rate: u32,
    frames: usize,
    host_bin: &std::path::Path,
    routes: &[PluginRoute],
    sample: i64,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    let (mut control, mut audio) = playback_pair_at(project, rate, frames, sample)?;
    let chain = chain_of(project, &audio.current);
    control.hosts = HostGraph::open(host_bin, routes, &chain, frames, rate)?;
    control.save_controls = control.hosts.save_controls();
    audio.hosts = Box::new(control.hosts.audio_snapshot());
    control.chain = chain;
    control.routes = routes.to_vec();
    control.host_bin = host_bin.to_path_buf();
    Ok((control, audio))
}

pub fn playback_with_clock(
    project: &Project,
    rate: u32,
    frames: usize,
    clock: Box<dyn ClockSource>,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    // Queue depths limit pending replacements/commands, never tracks or notes.
    let (plans, incoming) = RingBuffer::new(2);
    let (returning, returns) = RingBuffer::new(2);
    let (commands, actions) = RingBuffer::new(32);
    let mut ids = PlaybackIds::new();
    let mut meters = BTreeMap::new();
    let caches = BTreeMap::new();
    let mut current = Box::new(Prepared::new(
        project,
        rate,
        frames,
        &mut ids,
        PreparedSources {
            clips: &[],
            caches: &caches,
        },
        None,
        &mut meters,
    )?);
    current.place(clock.position(), false)?;
    let reserved_events = current.events.len();
    let status = Arc::new(Status {
        generation: AtomicU32::new(0),
        playing: AtomicBool::new(false),
        sample: AtomicI64::new(clock.position()),
        tick: AtomicI64::new(
            current
                .tempo
                .sample_to_tick(clock.position(), rate)
                .unwrap_or(0),
        ),
        failed: AtomicBool::new(false),
        applied_command: AtomicU64::new(0),
        plugin_missed_blocks: AtomicU64::new(0),
        graph_unavailable_blocks: AtomicU64::new(0),
        requested_plan: AtomicU64::new(0),
        applied_plan: AtomicU64::new(0),
    });
    let control = PlaybackControl {
        plans,
        returns,
        commands,
        command_revision: 0,
        held: None,
        hosts: HostGraph::empty(),
        preparation_error: None,
        published_plan: 0,
        chain: Vec::new(),
        routes: Vec::new(),
        host_bin: std::path::PathBuf::new(),
        ids,
        meters,
        caches,
        rate,
        frames,
        status: status.clone(),
        clip_loader: Arc::new(empty_clips),
        project: project.clone(),
        loop_bounds: None,
        reserved_events,
        plugin_saves: Vec::new(),
        save_controls: Vec::new(),
    };
    let audio = PlaybackAudio {
        plans: incoming,
        returns: returning,
        commands: actions,
        applied_command: 0,
        reset_plugins: false,
        output_revision: 0,
        live_output: false,
        stop_tail: None,
        stop_fade_offset: None,
        fader_preview: None,
        clear_preview_after: None,
        hosts: Box::new(HostGraph::empty()),
        current,
        clock,
        playing: false,
        failed: false,
        loop_start: None,
        loop_end: None,
        metronome: false,
        spans: vec![
            Span {
                start: 0,
                frames: 0,
                block_offset: 0,
                release_at: None,
            };
            frames
        ],
        played: vec![
            PlayedSpan {
                start_sample: 0,
                frames: 0,
                block_offset: 0,
                release_at: None,
                loop_end: None,
                wrap_from: None,
            };
            frames
        ],
        clicks: vec![0.0; frames],
        status,
    };
    Ok((control, audio))
}

impl PlaybackControl {
    pub fn set_routes(&mut self, host_bin: &std::path::Path, routes: &[PluginRoute]) {
        self.host_bin = host_bin.to_path_buf();
        self.routes = routes.to_vec();
    }

    pub fn set_clip_loader(&mut self, loader: ClipLoader) {
        self.clip_loader = loader;
    }

    /// Stages sources for the next `replace`. Configure only while stopped,
    /// after all transport commands have been acknowledged. A caller must wait
    /// for the replacement plan before playing. Live crossfades and cache-key
    /// invalidation belong to the Phase 3 provider, not this prepared-source port.
    pub fn set_cached_tracks(&mut self, caches: &[CachedTrack]) -> Result<(), PlaybackError> {
        if self.status.playing.load(Ordering::Acquire)
            || self.status.applied_command.load(Ordering::Acquire) != self.command_revision
        {
            return Err(PlaybackError::Unsupported(
                "cached sources require an idle transport".into(),
            ));
        }
        let mut next = BTreeMap::new();
        for cache in caches {
            if cache.sample_rate != self.rate
                || cache
                    .right
                    .as_ref()
                    .is_some_and(|right| right.len() != cache.samples.len())
                || i64::try_from(cache.samples.len())
                    .ok()
                    .and_then(|len| cache.start_sample.checked_add(len))
                    .is_none()
                || !self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.id.raw() == cache.track)
                || next.insert(cache.track, cache.clone()).is_some()
            {
                return Err(PlaybackError::Media);
            }
        }
        self.caches = next;
        Ok(())
    }

    pub fn command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        match command {
            PlaybackCommand::PreviewFader { track, gain, pan }
                if !gain.is_finite()
                    || !(0.0..=4.0).contains(&gain)
                    || !pan.is_finite()
                    || !(-1.0..=1.0).contains(&pan)
                    || !self
                        .project
                        .tracks()
                        .iter()
                        .any(|entry| entry.id.raw() == track) =>
            {
                Err(PlaybackError::InvalidFader)
            }
            PlaybackCommand::SetLoop { start, end } if end > start => {
                self.set_loop(Some((start, end)))
            }
            PlaybackCommand::SetLoop { .. } | PlaybackCommand::ClearLoop => self.set_loop(None),
            other => {
                let revision = self.command_revision.wrapping_add(1);
                self.commands
                    .push((revision, other))
                    .map_err(|_| PlaybackError::BufferTooSmall)?;
                self.command_revision = revision;
                Ok(())
            }
        }
    }

    /// Receipt for the most recently queued transport command.
    pub fn command_revision(&self) -> u64 {
        self.command_revision
    }

    fn set_loop(&mut self, bounds: Option<(i64, i64)>) -> Result<(), PlaybackError> {
        let previous = std::mem::replace(&mut self.loop_bounds, bounds);
        let result = self.reprepare();
        if result.is_err() {
            self.loop_bounds = previous;
        }
        result
    }

    /// Event slots reserved for the current project and loop. Frame count does
    /// not scale this when no loop is set.
    pub fn reserved_events(&self) -> usize {
        self.reserved_events
    }

    /// Event slots reserved on each live plugin track.
    pub fn plugin_event_caps(&self) -> Vec<usize> {
        self.hosts.track_event_caps()
    }

    pub fn preview_fader(&mut self, track: u128, gain: f32, pan: f32) -> Result<(), PlaybackError> {
        self.command(PlaybackCommand::PreviewFader { track, gain, pan })
    }

    pub fn clear_fader_preview(&mut self, committed: bool) -> Result<(), PlaybackError> {
        let after_plan = if committed {
            self.status.requested_plan.load(Ordering::Acquire)
        } else {
            0
        };
        self.command(PlaybackCommand::ClearFaderPreview { after_plan })
    }

    pub fn meters(&self) -> Vec<Arc<TrackMeter>> {
        self.project
            .tracks()
            .iter()
            .filter_map(|track| self.meters.get(&track.id.raw()).cloned())
            .collect()
    }

    pub fn replace(&mut self, project: &Project) -> Result<(), PlaybackError> {
        let previous = std::mem::replace(&mut self.project, project.clone());
        let result = self.reprepare();
        if result.is_err() {
            self.project = previous;
        }
        result
    }

    fn reprepare(&mut self) -> Result<(), PlaybackError> {
        self.service();
        if let Some(error) = self.take_preparation_error() {
            return Err(error);
        }
        let loaded = (self.clip_loader)(&self.project, self.rate)?;
        let prepared = Prepared::new(
            &self.project,
            self.rate,
            self.frames,
            &mut self.ids,
            PreparedSources {
                clips: &loaded,
                caches: &self.caches,
            },
            self.loop_bounds,
            &mut self.meters,
        )?;
        self.meters
            .retain(|id, _| prepared.mixer_slot(*id).is_some());
        let chain = chain_of(&self.project, &prepared);
        let revision = self
            .status
            .requested_plan
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let previous_events = self.reserved_events;
        self.reserved_events = prepared.events.len();
        self.held = Some(Pending {
            prepared: Box::new(prepared),
            chain,
            routes: self.routes.clone(),
            revision,
        });
        let result = self.publish_pending();
        if result.is_err() {
            self.reserved_events = previous_events;
        }
        result
    }

    fn publish_pending(&mut self) -> Result<(), PlaybackError> {
        if self.plans.slots() == 0 {
            return Ok(());
        }
        let Some(pending) = &self.held else {
            return Ok(());
        };
        if !self.hosts.can_reconcile(&pending.chain) {
            return Ok(());
        }
        let pending = self.held.take().ok_or(PlaybackError::Plugin)?;
        if !self.host_bin.as_os_str().is_empty() {
            self.hosts.take_restarts();
            let checkpoint = self.hosts.preparation_checkpoint();
            if let Err(error) = self.hosts.reconcile(
                &self.host_bin,
                &pending.routes,
                &pending.chain,
                self.frames,
                self.rate,
            ) {
                self.status
                    .requested_plan
                    .store(self.published_plan, Ordering::Release);
                self.hosts.cancel_preparation(&checkpoint)?;
                return Err(error);
            }
            self.save_controls = self.hosts.save_controls();
        }
        self.chain = pending.chain;
        let update = Update {
            prepared: pending.prepared,
            hosts: Box::new(self.hosts.audio_snapshot()),
            revision: pending.revision,
        };
        // Only this control thread produces updates; the reserved slot remains free.
        self.plans
            .push(update)
            .map_err(|_| PlaybackError::BufferTooSmall)?;
        self.published_plan = pending.revision;
        Ok(())
    }

    pub fn service(&mut self) {
        while self.returns.pop().is_ok() {}
        for (device, control) in &mut self.save_controls {
            if let Some(bytes) = control.poll_state_save() {
                self.plugin_saves.push((*device, bytes));
            }
        }
        if let Err(error) = self.publish_pending() {
            self.preparation_error = Some(error);
        }
    }

    /// Report asynchronous preparation failure on the control/device path.
    pub fn take_preparation_error(&mut self) -> Option<PlaybackError> {
        self.preparation_error.take()
    }

    /// Saved plugin bytes waiting for the document thread. The caller journals
    /// them; a device that disappeared is the caller's to drop.
    pub fn take_plugin_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        std::mem::take(&mut self.plugin_saves)
    }
    /// Device id and reason when a plugin missed the block that just rendered.
    /// Live playback still silences that block and leaves [`PlaybackStatus::failed`]
    /// clear. Export uses this so a deadline miss is not a finished file.
    pub fn plugin_failure(&self) -> Option<String> {
        self.hosts.export_failure()
    }

    pub fn status(&self) -> PlaybackStatus {
        loop {
            let before = self.status.generation.load(Ordering::SeqCst);
            if !before.is_multiple_of(2) {
                continue;
            }
            let value = PlaybackStatus {
                playing: self.status.playing.load(Ordering::SeqCst),
                sample: self.status.sample.load(Ordering::SeqCst),
                tick: self.status.tick.load(Ordering::SeqCst),
                failed: self.status.failed.load(Ordering::SeqCst),
                applied_command: self.status.applied_command.load(Ordering::SeqCst),
                plugin_missed_blocks: self.status.plugin_missed_blocks.load(Ordering::Relaxed),
                pending_plan: self.status.requested_plan.load(Ordering::Acquire)
                    != self.status.applied_plan.load(Ordering::Acquire),
                graph_unavailable_blocks: self
                    .status
                    .graph_unavailable_blocks
                    .load(Ordering::Relaxed),
            };
            if before == self.status.generation.load(Ordering::SeqCst) {
                return value;
            }
        }
    }
}

impl PlaybackAudio {
    /// Changes when a transport command can replace the audible waveform.
    /// Only the callback owns this value; the device uses it to remove steps.
    pub fn output_revision(&self) -> u64 {
        self.output_revision
    }

    pub fn has_live_output(&self) -> bool {
        self.live_output
    }
    pub fn current_sample(&self) -> i64 {
        self.clock.position()
    }
    pub fn stop_fade_offset(&self) -> Option<u32> {
        self.stop_fade_offset
    }

    /// Cumulative helper failures, sampled with the submitted output for diagnostics.
    pub fn plugin_missed_blocks(&self) -> u64 {
        self.status.plugin_missed_blocks.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn cleanup_pending(&self) -> bool {
        self.hosts.cleanup_pending()
    }

    /// Live stop renders only already sounding voices and placements for 5 ms.
    /// The published transport clock stops at the boundary; offline stop is immediate.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render_live_gated_until(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
    ) {
        let _ = self.render_mode(left, right, deadline, enabled, true);
    }

    /// A start cancelled before any device block was heard cannot emit a tail.
    pub fn cancel_unheard_start(&mut self, sample: i64) {
        self.clock.locate(sample);
        self.playing = false;
        self.stop_tail = None;
        self.stop_fade_offset = None;
        self.live_output = false;
        if self.finish_stop().is_err() {
            self.failed = true;
        }
        self.publish_status();
    }

    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let deadline = crate::plugins::callback_deadline(left.len(), self.current.sample_rate);
        self.render_until(left, right, deadline);
    }

    /// Same callback as [`Self::render`], with the absolute deadline already
    /// chosen. The device path stamps that deadline at entry. A test that has
    /// already spent the block passes a deadline in the past.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render_until(&mut self, left: &mut [f32], right: &mut [f32], deadline: Instant) {
        let _ = self.render_checked(left, right, deadline);
    }

    /// A cancelled transport still drains prepared plans and commands while
    /// holding the sample clock and producing silence. The device's atomic
    /// cancellation gate need not wait for its control worker to finish IO.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render_gated_until(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
    ) {
        let _ = self.render_checked_gated(left, right, deadline, enabled);
    }

    pub(crate) fn render_checked(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
    ) -> Result<(), PlaybackError> {
        self.render_checked_gated(left, right, deadline, true)
    }

    fn render_checked_gated(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
    ) -> Result<(), PlaybackError> {
        self.render_mode(left, right, deadline, enabled, false)
    }

    fn render_mode(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
        live: bool,
    ) -> Result<(), PlaybackError> {
        let _guard = ForbidAlloc::enter();
        left.fill(0.0);
        right.fill(0.0);
        let release = TrackMeter::release(left.len(), self.current.sample_rate);
        for track in &self.current.tracks {
            track.meter.publish(0.0, release);
        }
        let result = self.step(left, right, deadline, enabled, live);
        if result.is_err() {
            self.playing = false;
            self.failed = true;
            left.fill(0.0);
            right.fill(0.0);
        }
        self.publish_status();
        result
    }

    fn publish_status(&self) {
        self.status.generation.fetch_add(1, Ordering::SeqCst);
        self.status
            .sample
            .store(self.clock.position(), Ordering::SeqCst);
        self.status.tick.store(
            self.current
                .tempo
                .sample_to_tick(self.clock.position(), self.current.sample_rate)
                .unwrap_or(0),
            Ordering::SeqCst,
        );
        self.status.playing.store(
            self.playing && !self.hosts.cleanup_pending(),
            Ordering::SeqCst,
        );
        self.status.failed.store(self.failed, Ordering::SeqCst);
        self.status
            .applied_command
            .store(self.applied_command, Ordering::SeqCst);
        self.status.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn step(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
        live: bool,
    ) -> Result<(), PlaybackError> {
        if left.len() != right.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
        self.stop_fade_offset = None;
        let mut faders_changed = false;
        let previous_fader = self.fader_preview.map(|entry| entry.0);
        if self.stop_tail.is_none()
            && !self.reset_plugins
            && !self.hosts.cleanup_pending()
            && self.returns.slots() > 0
            && let Ok(mut next) = self.plans.pop()
        {
            let carried =
                next.prepared
                    .carry(&mut self.current, self.clock.position(), self.playing);
            if carried.is_ok() {
                next.hosts.carry(&self.hosts);
                next.hosts.activate_prepared();
                std::mem::swap(&mut next.prepared, &mut self.current);
                std::mem::swap(&mut next.hosts, &mut self.hosts);
                faders_changed = true;
                self.status
                    .applied_plan
                    .store(next.revision, Ordering::Release);
            }
            if let Err(rtrb::PushError::Full(_)) = self.returns.push(next) {
                std::process::abort();
            }
            carried?;
        }
        self.loop_start = self.current.loop_start;
        self.loop_end = self.current.loop_end;
        while let Ok((revision, command)) = self.commands.pop() {
            match command {
                PlaybackCommand::Play => {
                    if self.stop_tail.is_some() {
                        self.finish_stop()?;
                    }
                    self.output_revision = self.output_revision.wrapping_add(1);
                    self.playing = true;
                    self.failed = false;
                    self.current.place(self.clock.position(), false)?;
                }
                PlaybackCommand::Pause | PlaybackCommand::Stop => {
                    self.output_revision = self.output_revision.wrapping_add(1);
                    self.begin_stop(live)?;
                }
                PlaybackCommand::Locate(sample) => {
                    if self.stop_tail.is_some() {
                        self.finish_stop()?;
                    }
                    self.output_revision = self.output_revision.wrapping_add(1);
                    self.clock.locate(sample);
                    self.current.place(sample, true)?;
                    self.reset_plugins = true;
                }
                PlaybackCommand::SetLoop { start, end } if end > start => {
                    self.loop_start = Some(start);
                    self.loop_end = Some(end);
                }
                PlaybackCommand::SetLoop { .. } | PlaybackCommand::ClearLoop => {
                    self.loop_start = None;
                    self.loop_end = None;
                }
                PlaybackCommand::SetMetronome(enabled) => self.metronome = enabled,
                PlaybackCommand::PreviewFader { track, gain, pan } => {
                    self.fader_preview = Some((track, gain, pan));
                    self.clear_preview_after = None;
                    faders_changed = true;
                }
                PlaybackCommand::ClearFaderPreview { after_plan } => {
                    self.clear_preview_after = Some(after_plan)
                }
            }
            self.applied_command = revision;
        }
        if self.clear_preview_after.is_some_and(|revision| {
            revision == 0
                || self
                    .status
                    .applied_plan
                    .load(Ordering::Acquire)
                    .wrapping_sub(revision)
                    <= u64::MAX / 2
        }) {
            self.fader_preview = None;
            self.clear_preview_after = None;
            faders_changed = true;
        }
        if faders_changed {
            self.current.preview_fader(
                previous_fader,
                self.fader_preview,
                self.playing && (self.live_output || !live),
            );
        }
        if !enabled && self.playing {
            self.output_revision = self.output_revision.wrapping_add(1);
            self.begin_stop(live)?;
        }
        if let Some(tail) = self.stop_tail {
            self.render_stop_tail(tail, left, right, deadline)?;
            return Ok(());
        }
        if self.reset_plugins {
            if live || self.hosts.cleanup_pending() {
                self.current.queue_cleanup(&mut self.hosts)?;
            } else {
                self.hosts.reset_processing();
            }
            self.reset_plugins = false;
        }
        if self.hosts.cleanup_pending() {
            let cleanup_deadline = if live { None } else { Some(deadline) };
            let completed = self.hosts.collect_cleanup(cleanup_deadline);
            if self.hosts.missed_block() {
                self.status
                    .plugin_missed_blocks
                    .fetch_add(1, Ordering::Relaxed);
            }
            if !completed {
                return Ok(());
            }
            if self.playing {
                self.output_revision = self.output_revision.wrapping_add(1);
            }
        }
        if !self.playing {
            if !live {
                let flushed = self.current.released != 0;
                self.current.flush_releases(&mut self.hosts, deadline)?;
                if flushed && self.hosts.missed_block() {
                    self.status
                        .plugin_missed_blocks
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            return Ok(());
        }
        let (span_count, end) = split_loop(
            self.clock.position(),
            left.len(),
            self.loop_start,
            self.loop_end,
            &mut self.spans,
        )?;
        let spans = self
            .spans
            .get(..span_count)
            .ok_or(PlaybackError::BufferTooSmall)?;
        self.current.render(
            spans,
            left,
            right,
            &mut self.hosts,
            deadline,
            RenderRange {
                loop_bounds: (self.loop_start, self.loop_end),
                started_before: None,
            },
        )?;
        if self.hosts.missed_block() {
            self.status
                .plugin_missed_blocks
                .fetch_add(1, Ordering::Relaxed);
        }
        if self.metronome {
            mix_metronome(self, span_count, left, right, None)?;
        }
        self.clock.locate(end);
        self.live_output |= live;
        Ok(())
    }

    fn begin_stop(&mut self, live: bool) -> Result<(), PlaybackError> {
        if live && self.live_output && self.playing && self.stop_tail.is_none() {
            self.stop_tail = Some(StopTail {
                position: self.clock.position(),
                cutoff: self.clock.position(),
                remaining: (self.current.sample_rate / 200).max(2),
                loop_bounds: (self.loop_start, self.loop_end),
            });
        }
        self.playing = false;
        if self.stop_tail.is_none() || !live {
            self.finish_stop()?;
        }
        Ok(())
    }

    fn finish_stop(&mut self) -> Result<(), PlaybackError> {
        self.stop_tail = None;
        self.live_output = false;
        self.current.place(self.clock.position(), true)?;
        self.reset_plugins = true;
        for track in &mut self.current.tracks {
            track.fader.settle();
        }
        Ok(())
    }

    fn render_stop_tail(
        &mut self,
        mut tail: StopTail,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
    ) -> Result<(), PlaybackError> {
        let frames = left.len().min(tail.remaining as usize);
        self.stop_fade_offset = Some((self.current.sample_rate / 200).max(2) - tail.remaining);
        let span = Span {
            start: tail.position,
            frames: frames as u32,
            block_offset: 0,
            release_at: None,
        };
        self.current.render(
            &[span],
            left.get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?,
            right
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?,
            &mut self.hosts,
            deadline,
            RenderRange {
                loop_bounds: tail.loop_bounds,
                started_before: Some(tail.cutoff),
            },
        )?;
        if self.metronome {
            *self
                .spans
                .first_mut()
                .ok_or(PlaybackError::BufferTooSmall)? = span;
            mix_metronome(
                self,
                1,
                left.get_mut(..frames)
                    .ok_or(PlaybackError::BufferTooSmall)?,
                right
                    .get_mut(..frames)
                    .ok_or(PlaybackError::BufferTooSmall)?,
                Some(tail.cutoff),
            )?;
        }
        if self.hosts.missed_block() {
            self.status
                .plugin_missed_blocks
                .fetch_add(1, Ordering::Relaxed);
        }
        tail.remaining -= frames as u32;
        tail.position = tail
            .position
            .checked_add(frames as i64)
            .ok_or(PlaybackError::Time)?;
        if tail.remaining == 0 {
            self.finish_stop()?;
        } else {
            self.stop_tail = Some(tail);
        }
        Ok(())
    }
}

fn split_loop(
    mut position: i64,
    frames: usize,
    loop_start: Option<i64>,
    loop_end: Option<i64>,
    spans: &mut [Span],
) -> Result<(usize, i64), PlaybackError> {
    let mut budget = u32::try_from(frames).map_err(|_| PlaybackError::BufferTooSmall)?;
    let mut offset = 0u32;
    let mut count = 0usize;
    while budget > 0 {
        let mut release_at = None;
        if let (Some(start), Some(end)) = (loop_start, loop_end)
            && position >= end
        {
            release_at = Some(end);
            position = start;
        }
        let until = match loop_end {
            Some(end) if end > position => u32::try_from(end - position).unwrap_or(u32::MAX),
            Some(_) => 0,
            None => u32::MAX,
        };
        let span_frames = budget.min(until);
        if span_frames == 0 {
            return Err(PlaybackError::InvalidLoop);
        }
        let slot = spans.get_mut(count).ok_or(PlaybackError::BufferTooSmall)?;
        *slot = Span {
            start: position,
            frames: span_frames,
            block_offset: offset,
            release_at,
        };
        offset = offset.checked_add(span_frames).ok_or(PlaybackError::Time)?;
        budget -= span_frames;
        position = position
            .checked_add(i64::from(span_frames))
            .ok_or(PlaybackError::Time)?;
        count += 1;
    }
    Ok((count, position))
}

fn mix_metronome(
    audio: &mut PlaybackAudio,
    span_count: usize,
    left: &mut [f32],
    right: &mut [f32],
    cutoff: Option<i64>,
) -> Result<(), PlaybackError> {
    for index in 0..span_count {
        let span = *audio
            .spans
            .get(index)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let played = audio
            .played
            .get_mut(index)
            .ok_or(PlaybackError::BufferTooSmall)?;
        *played = PlayedSpan {
            start_sample: span.start,
            frames: span.frames,
            block_offset: span.block_offset,
            release_at: span.release_at,
            loop_end: audio.loop_end,
            wrap_from: span.release_at.and(audio.loop_start),
        };
    }
    audio
        .clicks
        .get_mut(..left.len())
        .ok_or(PlaybackError::BufferTooSmall)?
        .fill(0.0);
    render_metronome_index_before(
        audio
            .played
            .get(..span_count)
            .ok_or(PlaybackError::BufferTooSmall)?,
        &audio.current.tempo,
        audio.current.meter,
        audio.current.sample_rate,
        audio
            .clicks
            .get_mut(..left.len())
            .ok_or(PlaybackError::BufferTooSmall)?,
        cutoff,
    )?;
    let clicks = audio
        .clicks
        .get(..left.len())
        .ok_or(PlaybackError::BufferTooSmall)?;
    for ((sample_left, sample_right), click) in left.iter_mut().zip(right.iter_mut()).zip(clicks) {
        *sample_left += click;
        *sample_right += click;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    fn placed(track: u128, start: i64, value: f32, frames: usize) -> LoadedClip {
        LoadedClip {
            track,
            start_sample: start,
            samples: Arc::from(vec![value; frames]),
            right: None,
            fade_in: 0,
            fade_out: 0,
        }
    }

    #[test]
    fn stereo_clips_loop_and_fader_replacements_run_under_rtsan() {
        let (project, track) = Project::new().add_track("stereo");
        let gain = project.tracks()[0].devices[0]
            .parameters
            .iter()
            .find(|parameter| parameter.key == echode_model::ParamKey::Gain)
            .unwrap()
            .id;
        let quiet = project.set_parameter(gain, 0.0).unwrap();
        let block = 64; // Scratch for this test, not a playback limit.
        let (mut control, mut audio) = playback_pair(&project, 48_000, block).unwrap();
        control.set_clip_loader(Arc::new(move |_, _| {
            let mut clip = placed(track.raw(), 0, 0.25, 2048);
            clip.right = Some(Arc::from(vec![-0.25; 2048]));
            Ok(vec![clip])
        }));
        control
            .command(PlaybackCommand::SetLoop {
                start: 337,
                end: 1360,
            })
            .unwrap();
        control.command(PlaybackCommand::Locate(337)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        for step in 0..32 {
            if step == 7 || step == 8 {
                control.replace(&quiet).unwrap();
            }
            audio.render(&mut left, &mut right);
            assert!(!control.status().failed);
            assert!(
                left.iter()
                    .zip(&right)
                    .all(|(left, right)| *left == -*right)
            );
            if step >= 11 {
                assert!(left.iter().all(|sample| *sample == 0.0));
            }
        }
    }

    #[test]
    fn state_polling_does_not_change_the_current_plan() {
        let (mut control, mut audio) = playback_pair(&Project::new(), 48000, 64).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = [0.0; 64];
        let mut right = left;
        for _ in 0..8 {
            control.service();
            audio.render(&mut left, &mut right);
        }
        assert_eq!(control.status().sample, 512);
        assert_eq!(control.status().graph_unavailable_blocks, 0);
    }

    fn callback_block(mut clips: Vec<LoadedClip>) -> Vec<f32> {
        let project = Project::new().add_track("file").0;
        let track = project.tracks()[0].id.raw();
        for clip in &mut clips {
            clip.track = track;
        }
        let (mut control, mut audio) = playback_pair(&project, 48_000, 256).unwrap();
        control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = vec![0.0; 256];
        let mut right = vec![0.0; 256];
        audio.render(&mut left, &mut right);
        assert!(!control.status().failed);
        left
    }

    #[test]
    fn audit_aud12_future_clips_leave_the_callback_unchanged() {
        let body = placed(0, 0, 0.25, 256);
        let alone = callback_block(vec![body.clone()]);
        let mut crowded = vec![body];
        for index in 0..8_000 {
            crowded.push(placed(0, 1_000_000 + i64::from(index) * 300, 0.9, 256));
        }
        assert_eq!(callback_block(crowded), alone);
        assert!(alone.iter().any(|sample| *sample != 0.0));
        assert!(
            callback_block(vec![placed(0, -256, 0.9, 256)])
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert!(
            callback_block(vec![placed(0, 256, 0.9, 256)])
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    #[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
    fn future_clips_do_not_extend_the_callback() {
        const CALLBACKS: u32 = 2_000;
        let time = |future: usize| {
            let mut clips = vec![placed(0, 0, 0.25, 256)];
            for index in 0..future {
                clips.push(placed(
                    0,
                    1_000_000_000 + i64::try_from(index).unwrap_or(i64::MAX) * 300,
                    0.9,
                    64,
                ));
            }
            let project = Project::new().add_track("file").0;
            let track = project.tracks()[0].id.raw();
            for clip in &mut clips {
                clip.track = track;
            }
            let (mut control, mut audio) = playback_pair(&project, 48_000, 256).unwrap();
            control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut left = vec![0.0; 256];
            let mut right = vec![0.0; 256];
            audio.render(&mut left, &mut right);
            let started = Instant::now();
            for _ in 0..CALLBACKS {
                audio.render(&mut left, &mut right);
            }
            started.elapsed()
        };
        let near = time(0);
        let far = time(100_000);
        println!(
            "{CALLBACKS} callbacks, 1 sounding clip: {near:?}; plus 100000 future clips: {far:?}"
        );
        let ratio = far.as_secs_f64() / near.as_secs_f64().max(1.0e-6);
        assert!(
            ratio < 3.0,
            "callback time grew {ratio:.2}× with clips that do not sound"
        );
    }

    #[test]
    fn ten_thousand_product_replacements_and_transport_commands_run_under_rtsan() {
        let project = Project::new().add_track("silent").0;
        let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        for index in 0..10000 {
            control.replace(&project).unwrap();
            control
                .command(match index % 4 {
                    0 => PlaybackCommand::Play,
                    1 => PlaybackCommand::Pause,
                    2 => PlaybackCommand::Locate(index),
                    _ => PlaybackCommand::Stop,
                })
                .unwrap();
            audio.render(&mut left, &mut right);
            assert!(!control.status().failed);
        }
    }

    fn project_with_notes(count: usize) -> Project {
        use echode_model::{Id, NoteDraft, Tick};
        let (project, track) = Project::new().add_track("piano");
        if count == 0 {
            return project;
        }
        let (project, _) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(1),
                },
            )
            .unwrap();
        let mut content = project.contents()[0].as_ref().clone();
        let note = content.notes[0].clone();
        // Fixture construction is outside the measured playback preparation.
        // Avoid rebuilding/cloning the growing model once per note. Assemble
        // once through the public invariant checker with the same note geometry.
        content.notes = (0..count)
            .map(|index| echode_model::Note {
                id: Id::generate(),
                start: Tick::new(index as i64 * 48_000),
                ..note.clone()
            })
            .collect();
        let mut track = project.tracks()[0].as_ref().clone();
        let end = (count as i64 - 1) * 48_000 + 1;
        let bar = project.meter().bar_ticks().unwrap().get();
        track.clips[0].length = Tick::new((end + bar - 1).div_euclid(bar) * bar);
        Project::try_assemble(
            project.meter(),
            project.tempo().clone(),
            vec![content],
            vec![track],
        )
        .unwrap()
    }

    #[test]
    fn audit_rea01_a_loop_free_plan_does_not_scale_events_with_the_block() {
        for notes in [100usize, 1_000] {
            let project = project_with_notes(notes);
            let mut counts = Vec::new();
            for frames in [1usize, 64, 128, 512] {
                let (control, _audio) = playback_pair(&project, 48_000, frames).unwrap();
                counts.push(control.reserved_events());
            }
            assert!(
                counts.iter().all(|count| *count == counts[0]),
                "{notes} notes scaled with the block: {counts:?}"
            );
            assert!(
                counts[0] < notes.saturating_mul(8),
                "{notes} reserved {}",
                counts[0]
            );
        }
        use echode_model::{NoteDraft, Tick};
        let (project, track) = Project::new().add_track("pad");
        let project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                },
            )
            .unwrap()
            .0;
        let (mut control, _audio) = playback_pair(&project, 48_000, 512).unwrap();
        let open = control.reserved_events();
        control
            .command(PlaybackCommand::SetLoop { start: 0, end: 1 })
            .unwrap();
        assert!(control.reserved_events() > open);
        control.command(PlaybackCommand::ClearLoop).unwrap();
        assert_eq!(control.reserved_events(), open);
    }

    #[test]
    #[ignore = "timing measurement; `make spike-engine` runs it in a release build"]
    fn many_notes_reserve_events_without_the_block_length() {
        for notes in [10_000usize, 100_000] {
            let project = project_with_notes(notes);
            let small = playback_pair(&project, 48_000, 1)
                .unwrap()
                .0
                .reserved_events();
            let large = playback_pair(&project, 48_000, 512)
                .unwrap()
                .0
                .reserved_events();
            println!("events, {notes} notes, 1/512 frames: {small}/{large}");
            assert_eq!(small, large);
            assert!(large < notes.saturating_mul(8));
        }
    }

    #[test]
    fn audit_pro06_a_hundred_sample_loop_keeps_playing_in_a_large_block() {
        use echode_model::{NoteDraft, Tick};
        let (project, track) = Project::new().add_track("pad");
        let project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(4).unwrap(),
                },
            )
            .unwrap()
            .0;
        for frames in [64usize, 512] {
            let (mut control, mut audio) = playback_pair(&project, 48_000, frames).unwrap();
            control
                .command(PlaybackCommand::SetLoop { start: 0, end: 100 })
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut left = vec![0.0; frames];
            let mut right = vec![0.0; frames];
            audio.render(&mut left, &mut right);
            let peak = left
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.0, "{frames} frames were silent");
            let status = control.status();
            assert!(!status.failed, "{frames} frames failed");
            assert!(status.playing, "{frames} frames stopped");
        }
    }

    #[test]
    fn a_short_loop_wraps_and_the_metronome_clicks_on_the_downbeat() {
        use echode_model::{TICKS_PER_QUARTER, Tick};
        let project = Project::new();
        let (mut control, mut audio) = playback_pair(&project, 48_000, 64).unwrap();
        let quarter =
            crate::tick_to_sample(Tick::new(TICKS_PER_QUARTER), project.tempo(), 48_000).unwrap();
        control
            .command(PlaybackCommand::SetLoop {
                start: 0,
                end: quarter,
            })
            .unwrap();
        control
            .command(PlaybackCommand::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        audio.render(&mut left, &mut right);
        assert_ne!(left[0], 0.0);
        assert!(!control.status().failed);
        let mut rendered = 64i64;
        while rendered <= quarter {
            audio.render(&mut left, &mut right);
            rendered += 64;
            assert!(!control.status().failed);
        }
        assert!(control.status().sample < quarter);
    }

    #[test]
    fn audit_aud04_replacing_a_host_graph_and_sorting_events_does_not_allocate() {
        use crate::realtime::ForbidAlloc;
        use echode_realtime::{Event, sort_events};
        let mut events = Vec::with_capacity(1002);
        for index in 0..1000u32 {
            events.push(Event::note_on(1000 - index, 1, index, 60, 0.5));
        }
        events.insert(0, Event::note_on(40, 1, 7, 60, 0.2));
        events.insert(1, Event::note_on(40, 1, 7, 60, 0.8));
        events.insert(2, Event::note_off(40, 1, 7, 60));
        let mut order = vec![0usize; events.len()];
        let mut scratch = vec![Event::note_off(0, 0, 0, 0); events.len()];
        {
            let _guard = ForbidAlloc::enter();
            assert!(sort_events(&mut events, &mut order, &mut scratch));
        }
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].time() <= pair[1].time())
        );
        let at_forty: Vec<_> = events
            .iter()
            .filter(|event| event.time() == 40 && event.note_id() == Some(7))
            .map(|event| (event.is_note_off(), event.velocity()))
            .collect();
        assert_eq!(at_forty, vec![(true, 0.0), (false, 0.2), (false, 0.8)]);

        let project = Project::new().add_track("a").0;
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, 64, std::path::Path::new("/unused"), &[])
                .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        audio.render(&mut left, &mut right);
        let grown = project.add_track("b").0;
        control.replace(&grown).unwrap();
        {
            let _guard = ForbidAlloc::enter();
            audio.render(&mut left, &mut right);
        }
        control.replace(&project).unwrap();
        {
            let _guard = ForbidAlloc::enter();
            audio.render(&mut left, &mut right);
        }
        assert!(!control.status().failed);
    }
}
