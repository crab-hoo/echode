//! Product playback. All score, instrument, and scratch storage is prepared by
//! the control thread. The callback only moves prepared ownership through rings.

use crate::TrackMeter;
use crate::audio_log::{AudioLog, Note, NoteSender, Scene};
use crate::meter::AutoMute;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use echode_model::{Meter, Project};
use echode_realtime::{Cursor, Event, Plan, Span, TempoIndex, sort_events};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::mix::{FaderState, GateRamp, Signal};
use crate::plugins::{
    ChainDevice, HostGraph, PluginRoute, PluginWorkgroup, Progress, Submitted, TrackChain,
    collect_deadline,
};
use crate::realtime::ForbidAlloc;
use crate::sequence::render_metronome_index_before;
use crate::{PlaybackError, PlaybackIds, PlayedSpan, TrackFader};

#[path = "playback_capacity.rs"]
mod capacity;
use capacity::{TrackEventDemand, track_event_demand};
#[path = "playback_media.rs"]
mod media;
pub use media::MediaInputs;
use media::MediaPreparation;
#[path = "playback_preparation.rs"]
mod preparation;
use preparation::GraphPreparation;
#[path = "playback_faders.rs"]
mod faders;
use faders::{Routing, track_routing};
#[path = "playback_bus.rs"]
mod bus;
use crate::compensation::{TrackFlow, compensate};
use crate::delay::Delay;
use bus::Bus;
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

/// Control-side requests. Loop changes prepare plans and scalar previews become
/// owned groups before publication; this is not the audio ring's command type.
#[derive(Clone, Copy, Debug)]
pub enum PlaybackCommand {
    Play,
    Pause,
    Stop,
    Locate(i64),
    /// Stops and moves the clock to a device sample without resetting
    /// plugins. A stop that returns to the remembered start keeps tails and
    /// autonomous output, as a pause does (ADR 0155). Seeks use `Locate`.
    Rewind(i64),
    /// `Locate` to a project tick, converted under the tempo of the plan the
    /// callback plays when it takes the command. A sample the control computed
    /// from a newer project would land on another beat while that project's
    /// plan is still preparing (ADR 0330).
    LocateTick(i64),
    /// `Rewind` to a project tick, converted as `LocateTick` is.
    RewindTick(i64),
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

/// Why a device output muted the master by itself (ADR 0282). Starting
/// playback clears it. Offline rendering never sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MasterAutoMute {
    /// The first sample past [`MASTER_GUARD_DBFS`], or a NaN.
    pub peak: f32,
    /// Its sample: the block's start plus its frame. A loop within the block
    /// is not unwound; the value only tells the user where it happened.
    pub at: i64,
}

/// The master level a device output treats as a fault: +18 dBFS, eight times
/// full scale. The converter already clips at 0 dBFS, so only a signal that
/// has run away reaches this (ADR 0282).
pub const MASTER_GUARD_DBFS: f32 = 18.0;
/// `MASTER_GUARD_DBFS` as a linear amplitude.
const MASTER_GUARD: f32 = 7.943_282;

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
    pub master_auto_mute: Option<MasterAutoMute>,
    /// How far the adopted plan's master lags the timeline, in device frames,
    /// after plugin delay compensation (ADR 0339).
    pub latency_samples: u64,
}

struct Status {
    generation: AtomicU32,
    playing: AtomicBool,
    sample: AtomicI64,
    /// The current cursor's `chase_from`, `i64::MIN` for `None`. With `sample`
    /// it places the next plan in advance; a stale pair is only slower.
    chase_from: AtomicI64,
    tick: AtomicI64,
    failed: AtomicBool,
    applied_command: AtomicU64,
    plugin_missed_blocks: AtomicU64,
    graph_unavailable_blocks: AtomicU64,
    requested_plan: AtomicU64,
    applied_plan: AtomicU64,
    master_muted: AtomicBool,
    master_mute_peak: AtomicU32,
    master_mute_at: AtomicI64,
    latency: AtomicU64,
}

struct Track {
    key: u32,
    plane: Vec<f32>,
    right: Vec<f32>,
    /// Placed audio to mix with the first instrument before downstream effects.
    clip_plane: Vec<f32>,
    clip_right: Vec<f32>,
    clip_stereo: bool,
    stereo: bool,
    plugin_declick: crate::PluginDeclick,
    fader: FaderState,
    base_fader: TrackFader,
    /// Gate on the track's own placed audio and first instrument. Only a
    /// parent that passes a soloed child closes it (ADR 0338).
    own: GateRamp,
    /// One gain per frame for that gate. Every track has one: a parent that
    /// loses its children while gated still ramps its own audio back in.
    own_gain: Vec<f32>,
    /// `own_gain` holds this block's gains, so the source is gated.
    own_gated: bool,
    meter: Arc<TrackMeter>,
    cache: Option<CachedTrack>,
    clips: Vec<StoredClip>,
    /// The track's placed audio, delayed to meet its first instrument and
    /// its children before its next device (ADR 0339).
    source_delay: Delay,
    /// The track's output before its fader, delayed to meet its siblings at
    /// its parent or at master (ADR 0339).
    output_delay: Delay,
}

struct StoredClip {
    start_sample: i64,
    samples: Arc<[f32]>,
    right: Option<Arc<[f32]>>,
    /// Resolved [`ClipWindow`]: the plane index of the first frame, and the
    /// placement's length in frames.
    lead: i64,
    frames: usize,
    gain: f32,
    fade_in: u32,
    fade_out: u32,
}

/// The frames of a clip's planes that its placement plays.
///
/// Placement frame `i` reads plane index `lead + i`. Indices before the
/// planes or after their end are silence, so a trim, a negative offset, and
/// a tail past the source share one copy of the source PCM (EXT-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipWindow {
    pub lead: i64,
    pub frames: usize,
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
    /// The part of the planes this placement plays. `None` plays every
    /// frame from index 0, so the placement is as long as its planes.
    pub window: Option<ClipWindow>,
    /// Linear placement gain, applied before mono promotion and edge fades.
    /// The sample planes are unscaled and may be shared by other placements.
    pub gain: f32,
    pub fade_in: u32,
    pub fade_out: u32,
}

impl LoadedClip {
    /// The window this placement plays, with `None` resolved to the planes.
    pub fn played(&self) -> ClipWindow {
        self.window.unwrap_or(ClipWindow {
            lead: 0,
            frames: self.samples.len(),
        })
    }

    /// The unscaled left (or mono) sample at placement frame `index`.
    pub fn left_at(&self, index: usize) -> f32 {
        window_sample(&self.samples, self.played(), index)
    }

    /// The unscaled right sample at placement frame `index`; mono repeats left.
    pub fn right_at(&self, index: usize) -> f32 {
        window_sample(
            self.right.as_deref().unwrap_or(&self.samples),
            self.played(),
            index,
        )
    }
}

fn window_sample(plane: &[f32], window: ClipWindow, index: usize) -> f32 {
    if index >= window.frames {
        return 0.0;
    }
    i64::try_from(index)
        .ok()
        .and_then(|index| window.lead.checked_add(index))
        .and_then(|at| usize::try_from(at).ok())
        .and_then(|at| plane.get(at).copied())
        .unwrap_or(0.0)
}

/// A ready device-output plane, before the track fader. The provider owns
/// rendering/provenance/invalidation; the engine only selects and reads it.
/// A track with children never reads one: its output depends on the
/// children's bus, which a single plane cannot stand for (ADR 0338).
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
    /// Sort workspace, then the block's events grouped by track position.
    filtered: Vec<Event>,
    /// One more entry than `tracks`. See [`group_events`].
    event_ends: Vec<usize>,
    /// Index workspace for `sort_events`. Same length as `events`.
    sort_order: Vec<usize>,
    /// Note-offs already written at the front of `events`, not yet submitted.
    released: usize,
    previous_releases: usize,
    sample_rate: u32,
    meter: Meter,
    chase: bool,
    /// The plugin has been told about the cursor's sounding notes.
    plugin_holds: bool,
    /// Note-ons chased from the previous plan. Retained helpers hold these voices.
    /// A plugin that was just spawned still needs them. Timeline note-ons stay in `events`.
    /// Fixed length; the block's are `chased_ons[..chased_len]`.
    chased_ons: Vec<Event>,
    chased_len: usize,
    /// `chased_ons` grouped by track position, like `filtered` and `event_ends`.
    chased_grouped: Vec<Event>,
    chased_ends: Vec<usize>,
    /// Installed with the event buffers. The callback does not change these alone.
    loop_start: Option<i64>,
    loop_end: Option<i64>,
    demands: Vec<TrackEventDemand>,
    /// Which tracks feed which bus, and this block's start order.
    bus: Bus,
    /// One bus's sum of its children's post-fader audio. One bus is summed
    /// at a time, so every parent shares these.
    bus_left: Vec<f64>,
    bus_right: Vec<f64>,
    mix_left: Vec<f64>,
    mix_right: Vec<f64>,
    /// Set for the block once its post-fader peaks are stored. A callback
    /// publishes each meter once, after the block is mixed.
    meters_committed: bool,
    /// How far master lags the timeline after delay compensation (ADR 0339).
    latency: u64,
    /// The metronome, delayed by `latency` so it stays with the music.
    clicks_delay: Delay,
}

#[derive(Clone, Copy)]
struct RenderRange {
    loop_bounds: (Option<i64>, Option<i64>),
    started_before: Option<i64>,
    /// Fade only the score's audio source, before plugins. Their own release,
    /// delay and autonomous output remain audible after the transport stops.
    source_fade: Option<u32>,
    /// Release applied once, after the block is mixed. The coefficient covers
    /// the whole callback, including when only a prefix still reads score audio.
    meter_release: f32,
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
        let present: HashSet<u128> = project
            .tracks()
            .iter()
            .map(|track| track.id.raw())
            .collect();
        if loaded.iter().any(|clip| {
            !echode_model::clip_gain_allowed(clip.gain)
                || clip
                    .right
                    .as_ref()
                    .is_some_and(|right| right.len() != clip.samples.len())
                || !present.contains(&clip.track)
        }) {
            return Err(PlaybackError::Media);
        }
        // Unchanged tracks keep their plan, cursor lookups and demand (FCA-04).
        let crate::plan::Built { plan, derived } =
            crate::plan::build(project, project.tempo(), rate, ids)?;
        let cursor = plan
            .cursor_with(
                derived
                    .iter()
                    .map(|lookups| Arc::clone(&lookups.index))
                    .collect(),
            )
            .ok_or(PlaybackError::Time)?;
        let tempo = crate::plan::tempo_index(project.tempo())?;
        let Routing { gates, parents } = track_routing(project, &plan)?;
        let bus = Bus::new(parents);
        // Group the clips once, in loaded order, rather than filtering per track.
        let mut placed = HashMap::<u128, Vec<StoredClip>>::new();
        for clip in loaded {
            placed.entry(clip.track).or_default().push(StoredClip {
                start_sample: clip.start_sample,
                samples: Arc::clone(&clip.samples),
                right: clip.right.clone(),
                lead: clip.played().lead,
                frames: clip.played().frames,
                gain: clip.gain,
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
            });
        }
        let mut tracks = Vec::with_capacity(plan.tracks.len());
        let mut event_count = 1usize;
        let mut chase_count = 0usize;
        let mut demands = Vec::with_capacity(plan.tracks.len());
        for (position, (track, gate)) in plan.tracks.iter().zip(gates).enumerate() {
            let mut clips = placed.remove(&track.track).unwrap_or_default();
            clips.sort_by_key(|clip| clip.start_sample);
            // A parent's source includes its children's panned, stereo bus.
            let parent = bus.has_children(position);
            let clip_stereo = parent || clips.iter().any(|clip| clip.right.is_some());
            let demand = match ids.demand(track.track, loop_bounds, frames) {
                Some((events, chase)) => TrackEventDemand {
                    key: track.track_key,
                    events,
                    chase,
                    previous_chase: 0,
                },
                None => {
                    let demand = track_event_demand(track, loop_bounds, frames)?;
                    ids.remember_demand(track, loop_bounds, frames, demand.events, demand.chase);
                    demand
                }
            };
            event_count = event_count
                .checked_add(demand.events)
                .ok_or(PlaybackError::Time)?;
            chase_count = chase_count
                .checked_add(demand.chase)
                .ok_or(PlaybackError::Time)?;
            demands.push(demand);
            tracks.push(Track {
                key: track.track_key,
                meter: Arc::clone(
                    meters
                        .entry(track.track)
                        .or_insert_with(|| Arc::new(TrackMeter::new(track.track))),
                ),
                plane: vec![0.0; frames],
                right: vec![0.0; frames],
                clip_plane: vec![0.0; frames],
                clip_right: vec![0.0; frames],
                clip_stereo,
                stereo: clip_stereo,
                plugin_declick: crate::PluginDeclick::new(rate),
                fader: FaderState::new(gate.fader, rate),
                base_fader: gate.fader,
                own: GateRamp::new(gate.own, rate),
                own_gain: vec![1.0; frames],
                own_gated: false,
                cache: if parent {
                    None
                } else {
                    sources.caches.get(&track.track).cloned()
                },
                clips,
                source_delay: Delay::default(),
                output_delay: Delay::default(),
            });
        }
        event_count = event_count
            .checked_add(chase_count)
            .ok_or(PlaybackError::Time)?
            .max(1);
        let plan_tracks = tracks.len();
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
            event_ends: vec![0; plan_tracks + 1],
            sort_order: vec![0; event_count],
            released: 0,
            previous_releases: 0,
            sample_rate: rate,
            meter: project.meter(),
            chase: true,
            plugin_holds: false,
            chased_ons: vec![Event::note_off(0, 0, 0, 0); chase_count.max(1)],
            chased_len: 0,
            chased_grouped: vec![Event::note_off(0, 0, 0, 0); chase_count.max(1)],
            chased_ends: vec![0; plan_tracks + 1],
            loop_start: loop_bounds.map(|(start, _)| start),
            loop_end: loop_bounds.map(|(_, end)| end),
            demands,
            bus,
            bus_left: vec![0.0; frames],
            bus_right: vec![0.0; frames],
            mix_left: vec![0.0; frames],
            mix_right: vec![0.0; frames],
            meters_committed: false,
            latency: 0,
            clicks_delay: Delay::default(),
        })
    }

    fn mixer_slot(&self, id: u128) -> Option<usize> {
        self.mixer_indices
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()
            .and_then(|index| self.mixer_indices.get(index))
            .map(|entry| entry.1)
    }

    fn preview_faders(&mut self, preview: &[(u128, f32, f32)], restore: bool, playing: bool) {
        // The control thread validates and allocates the affected set. Never scan
        // the whole project or allocate/free group storage in the callback.
        for &(id, gain, pan) in preview {
            if let Some(slot) = self.mixer_slot(id)
                && let Some(track) = self.tracks.get_mut(slot)
            {
                let mut target = track.base_fader;
                if !restore {
                    target.gain = gain;
                    target.pan = pan;
                }
                track.fader.retarget(target);
                if !playing {
                    track.fader.settle();
                }
            }
        }
    }

    /// The device sample of a project tick under this plan's tempo. O(log T)
    /// over tempo boundaries, without allocating.
    fn sample_at(&self, tick: i64) -> Result<i64, PlaybackError> {
        self.tempo
            .tick_to_sample(tick, self.sample_rate)
            .ok_or(PlaybackError::Time)
    }

    fn place(&mut self, sample: i64, silence: bool) -> Result<(), PlaybackError> {
        if silence && self.plugin_holds {
            self.queue_all_offs()?;
            self.plugin_holds = false;
        }
        self.cursor.catch_up(&self.plan, sample);
        self.chase = true;
        Ok(())
    }

    /// The preceding published plan may release more voices than the new score
    /// can hold. Size this transition only after any background plan published.
    fn reserve_previous_releases(
        &mut self,
        previous: &BTreeMap<u32, usize>,
    ) -> Result<(), PlaybackError> {
        let extra = previous.values().try_fold(0usize, |sum, count| {
            sum.checked_add(*count).ok_or(PlaybackError::Time)
        })?;
        for demand in &mut self.demands {
            if let Some(&chase) = previous.get(&demand.key) {
                demand.events = demand
                    .events
                    .checked_add(chase.saturating_sub(demand.previous_chase))
                    .ok_or(PlaybackError::Time)?;
                demand.previous_chase = demand.previous_chase.max(chase);
            }
        }
        let size = self
            .events
            .len()
            .checked_add(extra.saturating_sub(self.previous_releases))
            .ok_or(PlaybackError::Time)?;
        self.previous_releases = self.previous_releases.max(extra);
        self.events.resize(size, Event::note_off(0, 0, 0, 0));
        self.filtered.resize(size, Event::note_off(0, 0, 0, 0));
        self.sort_order.resize(size, 0);
        Ok(())
    }

    /// Note-offs for every note this cursor has already sent to the plugin.
    fn queue_all_offs(&mut self) -> Result<(), PlaybackError> {
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
        let count = cursor.release_all(plan, 0, rest).map_err(play_error)?;
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
                track.plugin_declick = old.plugin_declick;
                // Audio already inside the compensation keeps its course (ADR 0339).
                track.source_delay.carry(&mut old.source_delay);
                track.output_delay.carry(&mut old.output_delay);
                if playing {
                    track.fader.carry(old.fader);
                    track.own.carry(old.own);
                }
            }
        }
        self.clicks_delay.carry(&mut previous.clicks_delay);
        // Unchanged tracks take the previous cursor's voices without a search.
        // Retained helpers hold the inherited chase; only new helpers strike it.
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
            .take_over(
                plan,
                &mut previous.cursor,
                &previous.plan,
                sample,
                previous.plugin_holds,
                rest,
            )
            .map_err(play_error)?;
        *released = start + count;
        self.chase = true;
        self.plugin_holds = previous.plugin_holds;
        Ok(())
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
        let mut count = self.released;
        self.released = 0;
        let count = if range.started_before.is_some() {
            count
        } else {
            self.chased_len = 0;
            if self.chase {
                self.chase = false;
                let rest = self
                    .events
                    .get_mut(count..)
                    .ok_or(PlaybackError::BufferTooSmall)?;
                let (fresh, inherited) = self
                    .cursor
                    .chase_events(&self.plan, rest, &mut self.chased_ons)
                    .map_err(play_error)?;
                count += fresh;
                self.chased_len = inherited;
            }
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
        let chased: &[Event] = if range.started_before.is_none() {
            self.chased_ons
                .get(..self.chased_len)
                .ok_or(PlaybackError::BufferTooSmall)?
        } else {
            &[]
        };
        if !group_events(
            events,
            &self.track_indices,
            &mut self.filtered,
            &mut self.event_ends,
        ) || !group_events(
            chased,
            &self.track_indices,
            &mut self.chased_grouped,
            &mut self.chased_ends,
        ) {
            return Err(PlaybackError::BufferTooSmall);
        }
        let frames = left.len();
        let rate = self.sample_rate;
        for track in &mut self.tracks {
            track.prepare_source(spans, range, rate, frames)?;
        }
        // One store after the block, including plugin completion. A decay written
        // before this point is what the UI samples for the whole callback.
        self.meters_committed = true;
        let Prepared {
            tracks,
            bus,
            filtered,
            event_ends,
            chased_grouped,
            chased_ends,
            track_indices,
            bus_left,
            bus_right,
            mix_left,
            mix_right,
            ..
        } = self;
        let mut block = Block {
            tracks,
            bus,
            hosts: &mut *hosts,
            events: filtered,
            event_ends,
            chased: chased_grouped,
            chased_ends,
            bus_left,
            bus_right,
            spans,
            range,
            frames,
            late: false,
            running: 0,
        };
        block.run(track_indices, collect_deadline(deadline, frames, rate))?;
        let mix_left = mix_left
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let mix_right = mix_right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        mix_left.fill(0.0);
        mix_right.fill(0.0);
        // Top-level tracks in plan order: a project without folders sums
        // exactly as it did before buses (ADR 0338).
        let Block { tracks, bus, .. } = block;
        for root in bus.roots() {
            tracks
                .get_mut(root)
                .ok_or(PlaybackError::BufferTooSmall)?
                .mix_into(mix_left, mix_right, frames, range.meter_release)?;
        }
        if hosts.dropped_events() {
            return Err(PlaybackError::BufferTooSmall);
        }
        for ((left, right), (l, r)) in left
            .iter_mut()
            .zip(right)
            .zip(mix_left.iter().zip(mix_right.iter()))
        {
            *left = finite_f32(*l);
            *right = finite_f32(*r);
        }
        self.plugin_holds |= range.started_before.is_none();
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
        if !group_events(
            events,
            &self.track_indices,
            &mut self.filtered,
            &mut self.event_ends,
        ) {
            return Err(PlaybackError::BufferTooSmall);
        }
        for (position, track) in self.tracks.iter_mut().enumerate() {
            let plane = track
                .plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            plane.fill(0.0);
            let filtered = track_events(&self.filtered, &self.event_ends, position)
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
                None,
            );
        }
        hosts.collect(collect_deadline(deadline, frames, self.sample_rate));
        Ok(())
    }

    /// Silences the compensation's delay lines at a discontinuity that also
    /// resets the plugins. O(1) per line.
    fn clear_delays(&mut self) {
        for track in &mut self.tracks {
            track.source_delay.clear();
            track.output_delay.clear();
        }
        self.clicks_delay.clear();
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

impl Track {
    /// Clears the planes and writes the track's own audio for the block:
    /// a cache into the output planes, or placed audio into the clip planes,
    /// gated when the track only passes a soloed child (ADR 0338).
    fn prepare_source(
        &mut self,
        spans: &[Span],
        range: RenderRange,
        rate: u32,
        frames: usize,
    ) -> Result<(), PlaybackError> {
        let Track {
            plane,
            right,
            clip_plane,
            clip_right,
            clip_stereo,
            stereo,
            clips,
            cache,
            own,
            own_gain,
            own_gated,
            source_delay,
            ..
        } = self;
        let plane = plane
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let right = right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let clip_plane = clip_plane
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let clip_right = clip_right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        plane.fill(0.0);
        right.fill(0.0);
        clip_plane.fill(0.0);
        clip_right.fill(0.0);
        *own_gated = false;
        if let Some(cache) = cache {
            read_cached_track(cache, spans, plane, right, range)?;
            fade_stopped_source(plane, right, range.source_fade, rate);
            *stereo = cache.right.is_some();
            return Ok(());
        }
        add_stored_clips(
            clips,
            spans,
            clip_plane,
            clip_right,
            *clip_stereo,
            range,
            rate,
        )?;
        fade_stopped_source(clip_plane, clip_right, range.source_fade, rate);
        // Late enough to meet the first instrument and the children; the gate
        // below then acts on what is heard, as it does on the instrument.
        source_delay.process(clip_plane, clip_stereo.then_some(&mut *clip_right));
        *stereo = *clip_stereo;
        if let Some(gains) = own_gain.get_mut(..frames)
            && own.fill(gains)
        {
            *own_gated = true;
            for ((left, right), gain) in clip_plane.iter_mut().zip(clip_right).zip(gains) {
                *left *= *gain;
                *right *= *gain;
            }
        }
        Ok(())
    }

    /// Adds this track's block through its fader and publishes its meter.
    /// Called once per block, by the parent's bus or by the master.
    fn mix_into(
        &mut self,
        left: &mut [f64],
        right: &mut [f64],
        frames: usize,
        release: f32,
    ) -> Result<(), PlaybackError> {
        let plane = self
            .plane
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let plane_right = self
            .right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        // Aligned with its siblings before the fader, so the fader, the gates
        // and the meter act on what is heard (ADR 0339).
        self.output_delay
            .process(plane, self.stereo.then_some(&mut *plane_right));
        let (plane, plane_right) = (&*plane, &*plane_right);
        let peak = self.fader.add(
            Signal {
                left: plane,
                right: self.stereo.then_some(plane_right),
            },
            left,
            right,
        );
        self.meter.publish(peak, release);
        Ok(())
    }
}

/// One block's tracks, started children first (ADR 0338).
///
/// Every track without children starts at once, so every helper runs in
/// parallel as before. A parent starts when its last child has its output:
/// its children's post-fader sum joins its clips, then its own chain runs.
/// After the deadline a parent is still submitted, so its helper hears the
/// block's events, and then abandoned like any late helper.
struct Block<'a> {
    tracks: &'a mut [Track],
    bus: &'a mut Bus,
    hosts: &'a mut HostGraph,
    events: &'a [Event],
    event_ends: &'a [usize],
    chased: &'a [Event],
    chased_ends: &'a [usize],
    bus_left: &'a mut [f64],
    bus_right: &'a mut [f64],
    spans: &'a [Span],
    range: RenderRange,
    frames: usize,
    late: bool,
    running: usize,
}

impl Block<'_> {
    fn run(&mut self, indices: &[(u32, usize)], deadline: Instant) -> Result<(), PlaybackError> {
        self.bus.begin();
        for position in 0..self.tracks.len() {
            if !self.bus.has_children(position) {
                self.start(position)?;
            }
        }
        self.start_ready()?;
        let mut seen = self.hosts.fan_seen();
        while self.running > 0 {
            self.bus.finished.clear();
            if self
                .hosts
                .progress(&mut self.bus.finished, &mut seen, deadline)
                == Progress::Late
            {
                self.late = true;
            }
            if self.bus.finished.is_empty() {
                // Nothing was pending although a track was counted as running.
                break;
            }
            let mut index = 0;
            while let Some(&key) = self.bus.finished.get(index) {
                index += 1;
                let Some(position) = indices
                    .binary_search_by_key(&key, |entry| entry.0)
                    .ok()
                    .and_then(|found| indices.get(found))
                    .map(|entry| entry.1)
                else {
                    continue;
                };
                self.running = self.running.saturating_sub(1);
                self.finish(position)?;
            }
            self.start_ready()?;
        }
        if self.running != 0 || !self.bus.complete() {
            return Err(PlaybackError::BufferTooSmall);
        }
        Ok(())
    }

    fn start_ready(&mut self) -> Result<(), PlaybackError> {
        while let Some(position) = self.bus.next_ready() {
            self.start(position)?;
        }
        Ok(())
    }

    fn start(&mut self, position: usize) -> Result<(), PlaybackError> {
        self.bus.note_started();
        if self.bus.has_children(position) {
            self.sum_children(position)?;
        }
        let frames = self.frames;
        let track = self
            .tracks
            .get_mut(position)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let submitted = if track.cache.is_some() {
            Submitted::Unhosted
        } else {
            let Track {
                key,
                plane,
                right,
                clip_plane,
                clip_right,
                clip_stereo,
                stereo,
                own_gain,
                own_gated,
                ..
            } = track;
            let plane = plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let right = right
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let clip_plane = clip_plane
                .get(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let clip_right = clip_right
                .get(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            plane.copy_from_slice(clip_plane);
            right.copy_from_slice(clip_right);
            *stereo = *clip_stereo;
            let events = track_events(self.events, self.event_ends, position)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let chased = track_events(self.chased, self.chased_ends, position)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let own = if *own_gated {
                own_gain.get(..frames)
            } else {
                None
            };
            let submitted = self.hosts.submit(
                *key,
                Signal {
                    left: plane,
                    right: stereo.then_some(&*right),
                },
                Signal {
                    left: clip_plane,
                    right: clip_stereo.then_some(clip_right),
                },
                events,
                chased,
                own,
            );
            if self.late && submitted == Submitted::Pending {
                self.hosts.abandon_track(*key);
                Submitted::Done
            } else {
                submitted
            }
        };
        if submitted == Submitted::Pending {
            self.running = self.running.saturating_add(1);
            Ok(())
        } else {
            self.finish(position)
        }
    }

    /// Adds each child's post-fader block to the parent's clip planes, in
    /// plan order, and publishes the children's meters.
    fn sum_children(&mut self, position: usize) -> Result<(), PlaybackError> {
        let frames = self.frames;
        let release = self.range.meter_release;
        let sum_left = self
            .bus_left
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let sum_right = self
            .bus_right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        sum_left.fill(0.0);
        sum_right.fill(0.0);
        for child in self.bus.children_of(position) {
            self.tracks
                .get_mut(child)
                .ok_or(PlaybackError::BufferTooSmall)?
                .mix_into(sum_left, sum_right, frames, release)?;
        }
        let parent = self
            .tracks
            .get_mut(position)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let clip_plane = parent
            .clip_plane
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let clip_right = parent
            .clip_right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        for (sample, sum) in clip_plane.iter_mut().zip(sum_left.iter()) {
            *sample = finite_f32(f64::from(*sample) + *sum);
        }
        for (sample, sum) in clip_right.iter_mut().zip(sum_right.iter()) {
            *sample = finite_f32(f64::from(*sample) + *sum);
        }
        Ok(())
    }

    /// Takes the track's helper output, or keeps its source, and lets its
    /// parent start once every sibling is done.
    fn finish(&mut self, position: usize) -> Result<(), PlaybackError> {
        let frames = self.frames;
        let track = self
            .tracks
            .get_mut(position)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let plane = track
            .plane
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        let track_right = track
            .right
            .get_mut(..frames)
            .ok_or(PlaybackError::BufferTooSmall)?;
        if track.cache.is_none()
            && let Some(applied) = self.hosts.apply(
                track.key,
                plane,
                track_right,
                &mut track.plugin_declick,
                track.meter.auto_muted(),
            )
        {
            track.stereo = applied.stereo;
            if let Some(fault) = applied.fault {
                track.meter.set_auto_mute(AutoMute {
                    device: fault.device,
                    at: block_sample(self.spans, self.range.started_before, fault.frame),
                });
            }
        } else {
            track.plugin_declick.reset();
        }
        self.bus.finish(position);
        Ok(())
    }
}

/// Groups one block's events by track position in one counting pass and one
/// placing pass, so the work grows with the events plus the tracks rather than
/// their product (XCA-13). Each track keeps its events in the block's order.
/// Afterwards track `i` owns `grouped[ends[i - 1]..ends[i]]`, from 0 for the
/// first track. Events of no listed track, such as transport, are left out.
fn group_events(
    events: &[Event],
    tracks: &[(u32, usize)],
    grouped: &mut [Event],
    ends: &mut [usize],
) -> bool {
    let position = |event: &Event| {
        let key = event.track_key()?;
        let index = tracks.binary_search_by_key(&key, |entry| entry.0).ok()?;
        tracks.get(index).map(|entry| entry.1)
    };
    ends.fill(0);
    for event in events {
        if let Some(slot) = position(event) {
            let Some(count) = ends.get_mut(slot) else {
                return false;
            };
            *count += 1;
        }
    }
    // Counts become starts. Placing advances each start to that track's end.
    let mut start = 0usize;
    for entry in ends.iter_mut() {
        let count = *entry;
        *entry = start;
        start += count;
    }
    for event in events {
        if let Some(slot) = position(event) {
            let Some(end) = ends.get_mut(slot) else {
                return false;
            };
            let Some(target) = grouped.get_mut(*end) else {
                return false;
            };
            *target = *event;
            *end += 1;
        }
    }
    true
}

/// The project sample of block frame `frame`: from the span that holds it
/// while playing, or the held position while stopped.
fn block_sample(spans: &[Span], held: Option<i64>, frame: usize) -> i64 {
    let frame = frame as i64;
    spans
        .iter()
        .find(|span| {
            let offset = i64::from(span.block_offset);
            frame >= offset && frame < offset + i64::from(span.frames)
        })
        .map(|span| span.start + frame - i64::from(span.block_offset))
        .or(held)
        .unwrap_or(0)
}

fn track_events<'a>(grouped: &'a [Event], ends: &[usize], position: usize) -> Option<&'a [Event]> {
    let start = match position.checked_sub(1) {
        Some(previous) => *ends.get(previous)?,
        None => 0,
    };
    grouped.get(start..*ends.get(position)?)
}

fn fade_stopped_source(left: &mut [f32], right: &mut [f32], offset: Option<u32>, rate: u32) {
    let Some(offset) = offset else { return };
    let frames = (rate / 200).max(2);
    for (index, (left, right)) in left.iter_mut().zip(right).enumerate() {
        let gain = crate::declick::stop_gain(u64::from(offset) + index as u64, frames);
        *left *= gain;
        *right *= gain;
    }
}

fn finite_f32(sample: f64) -> f32 {
    let sample = sample as f32;
    if sample.is_finite() { sample } else { 0.0 }
}

fn read_cached_track(
    cache: &CachedTrack,
    spans: &[Span],
    left: &mut [f32],
    right: &mut [f32],
    range: RenderRange,
) -> Result<(), PlaybackError> {
    // A flattened cache is one region. If it has not started, it stays silent
    // for the stop tail. Onsets already mixed into a cache that started
    // earlier cannot be separated here; the provider owns that content.
    if range
        .started_before
        .is_some_and(|cutoff| cache.start_sample >= cutoff)
    {
        return Ok(());
    }
    for span in spans {
        let mut end = span
            .start
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        if range.started_before.is_some()
            && let Some(loop_end) = range.loop_bounds.1
        {
            end = end.min(loop_end);
        }
        if end <= span.start {
            continue;
        }
        let frames = u32::try_from(end - span.start).map_err(|_| PlaybackError::Time)?;
        let played = [PlayedSpan {
            start_sample: span.start,
            frames,
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
            let promotion = if stereo && clip.right.is_none() {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            let mut view = [crate::voice::FadedClip {
                start_sample: clip.start_sample,
                samples: clip.samples.as_ref(),
                lead: clip.lead,
                frames: clip.frames,
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
            crate::voice::render_scaled_faded_clips(&view, &played, output, clip.gain, promotion)?;
            if stereo {
                view[0].samples = clip.right.as_deref().unwrap_or(&clip.samples);
                crate::voice::render_scaled_faded_clips(
                    &view, &played, right, clip.gain, promotion,
                )?;
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
        .saturating_add(i64::try_from(clip.frames).unwrap_or(i64::MAX))
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
                    if device.offline {
                        return None;
                    }
                    Some(ChainDevice {
                        id: device.id.raw(),
                        clap_id: device.plugin.clone()?,
                        bypassed: device.bypassed,
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

/// Plans delay compensation for `prepared` from one reading of the helpers'
/// reported latency and sizes its delay lines on this thread (ADR 0339,
/// ADR 0340). Returns the instrument delay of each hosted track for the audio
/// graph, and what was planned.
fn compensate_plan(
    prepared: &mut Prepared,
    hosts: &HostGraph,
    frames: usize,
) -> Result<(Vec<(u32, Delay)>, Compensated), PlaybackError> {
    let reported = hosts.latencies();
    let flows: Vec<TrackFlow<'_>> = prepared
        .tracks
        .iter()
        .zip(prepared.bus.parents())
        .map(|(track, parent)| TrackFlow {
            parent: *parent,
            // A cached track reads its finished audio and runs no device.
            clips: track.cache.is_none() && !track.clips.is_empty(),
            devices: reported.of(track.key),
        })
        .collect();
    if flows.len() != prepared.tracks.len() {
        return Err(PlaybackError::BufferTooSmall);
    }
    let plan = compensate(&flows)?;
    // Refused before anything is reserved: a helper may report any number.
    let lines = plan
        .tracks
        .iter()
        .flat_map(|delays| [delays.source, delays.instrument, delays.output])
        .chain([plan.clicks]);
    let mut footprint = 0u64;
    for delay in lines {
        footprint = footprint
            .checked_add(Delay::footprint(delay, frames)?)
            .ok_or(PlaybackError::Latency(plan.latency))?;
    }
    if footprint > crate::delay::budget() {
        return Err(PlaybackError::Latency(plan.latency));
    }
    let mut instruments = Vec::new();
    for (track, delays) in prepared.tracks.iter_mut().zip(&plan.tracks) {
        track.source_delay = Delay::new(delays.source, frames)?;
        track.output_delay = Delay::new(delays.output, frames)?;
        if delays.instrument != 0 {
            instruments.push((track.key, Delay::new(delays.instrument, frames)?));
        }
    }
    prepared.clicks_delay = Delay::new(plan.clicks, frames)?;
    prepared.latency = plan.latency;
    Ok((
        instruments,
        Compensated {
            reported: reported.reported(),
            latency: plan.latency,
            tail: plan.tail,
        },
    ))
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
    /// The tempo differs from the plan published before this one. Adopting it
    /// keeps the clock's tick instead of its sample (ADR 0330).
    retime: bool,
}

struct Pending {
    prepared: Box<Prepared>,
    chain: Vec<TrackChain>,
    routes: Vec<PluginRoute>,
    revision: u64,
    /// A loop-only preparation changes event storage, never plugin state.
    restore_states: bool,
}

type FaderGroup = Box<[(u128, f32, f32)]>;

/// Only commands ready for the audio thread may enter the ring. Loop changes
/// prepare plans and scalar fader requests become validated groups on control.
enum QueuedCommand {
    Play,
    Pause,
    Stop,
    Locate(i64),
    Rewind(i64),
    LocateTick(i64),
    RewindTick(i64),
    SetMetronome(bool),
    ClearFaderPreview { after_plan: u64 },
    Faders(FaderGroup),
}

pub struct PlaybackControl {
    workgroup: Option<crate::plugins::PluginWorkgroup>,
    plans: Producer<Update>,
    returns: Consumer<Update>,
    commands: Producer<(u64, QueuedCommand)>,
    preview_returns: Consumer<FaderGroup>,
    command_revision: u64,
    held: Option<Pending>,
    hosts: HostGraph,
    background: bool,
    preparing: Option<GraphPreparation>,
    preparation_error: Option<PlaybackError>,
    /// Replacements needing media not yet loaded load it on another thread.
    background_media: bool,
    media: Option<MediaPreparation>,
    /// What the clips most recently loaded were read from.
    media_inputs: Option<MediaInputs>,
    /// The replacement waiting for `media` restores recorded plugin states.
    media_restore_states: bool,
    /// The first plan with media loaded by `media`, until it is published.
    media_plan: Option<u64>,
    /// Counts project replacements, so loaded media knows whether it is newest.
    replacements: u64,
    published_plan: u64,
    /// The tempo of the plan most recently published to the callback.
    published_tempo: TempoIndex,
    chain: Vec<TrackChain>,
    /// Maximum live voices in the preceding published plan, including cached tracks.
    previous_chases: BTreeMap<u32, usize>,
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
    /// What the most recently published plan's delay compensation used.
    compensated: Compensated,
    /// Published plans whose compensation was planned from other latencies
    /// than the plan before them.
    compensation_changes: u64,
    /// The audio thread's notes, logged on this thread (ADR 0322).
    audio_log: AudioLog,
}

/// What a published plan's delay compensation was planned from (ADR 0339).
#[derive(Default)]
struct Compensated {
    /// Each hosted device's reported latency, in graph order, to notice a
    /// change that a helper's restart reports later.
    reported: Vec<(u128, u32)>,
    latency: u64,
    tail: u32,
}

impl Drop for PlaybackControl {
    /// Logs the counts still pending, so a closed output's last episode is not lost.
    fn drop(&mut self) {
        self.log_audio(true);
    }
}

/// Owned by exactly one callback. Move back to the control thread before Drop.
pub struct PlaybackAudio {
    plans: Consumer<Update>,
    returns: Producer<Update>,
    commands: Consumer<(u64, QueuedCommand)>,
    preview_returns: Producer<FaderGroup>,
    applied_command: u64,
    reset_plugins: bool,
    output_revision: u64,
    live_output: bool,
    /// The live graph has mixed a block, including while the score is stopped.
    /// Faders must ramp over that output without treating it as a heard Play.
    live_processed: bool,
    stop_tail: Option<StopTail>,
    stop_fade_offset: Option<u32>,
    fader_preview: Option<FaderGroup>,
    clear_preview_after: Option<u64>,
    hosts: Box<HostGraph>,
    current: Box<Prepared>,
    clock: Box<dyn ClockSource>,
    playing: bool,
    failed: bool,
    loop_start: Option<i64>,
    loop_end: Option<i64>,
    /// A locate at/after the end bypasses the loop until another locate or range change.
    loop_armed: bool,
    metronome: bool,
    /// One slot per prepared frame. A one-sample loop can fill every slot.
    spans: Vec<Span>,
    played: Vec<PlayedSpan>,
    clicks: Vec<f32>,
    status: Arc<Status>,
    /// See [`PlaybackOptions::guard_output`].
    guard_output: bool,
    master_mute: Option<MasterAutoMute>,
    /// Misses and late blocks for the control thread's log (ADR 0322).
    notes: NoteSender,
}

/// How [`playback`] starts. Every choice combines with the others, so a new
/// choice extends this type instead of adding another constructor.
pub struct PlaybackOptions<'a> {
    clock: Box<dyn ClockSource>,
    plugins: Option<PluginSetup<'a>>,
    workgroup: Option<PluginWorkgroup>,
    guard_output: bool,
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

struct PluginSetup<'a> {
    host_bin: &'a std::path::Path,
    routes: &'a [PluginRoute],
}

impl Default for PlaybackOptions<'_> {
    /// Stopped at sample zero on the internal clock, without helpers.
    fn default() -> Self {
        Self {
            clock: Box::<InternalClock>::default(),
            plugins: None,
            workgroup: None,
            guard_output: false,
            cancel: None,
        }
    }
}

impl<'a> PlaybackOptions<'a> {
    /// Prepares both the clock and the published position before a device can
    /// call back. Replaces any earlier [`Self::clock`].
    pub fn at(mut self, sample: i64) -> Self {
        self.clock = Box::new(InternalClock { position: sample });
        self
    }

    /// Replaces the internal clock and any earlier [`Self::at`].
    pub fn clock(mut self, clock: Box<dyn ClockSource>) -> Self {
        self.clock = clock;
        self
    }

    /// Starts a helper for each resolved CLAP device. Unresolved devices do not
    /// create a substitute instrument.
    pub fn plugins(mut self, host_bin: &'a std::path::Path, routes: &'a [PluginRoute]) -> Self {
        self.plugins = Some(PluginSetup { host_bin, routes });
        self
    }

    /// The selected output's retained workgroup, kept for this transport's
    /// lifetime so helpers started after opening join it too.
    pub fn workgroup(mut self, workgroup: PluginWorkgroup) -> Self {
        self.workgroup = Some(workgroup);
        self
    }

    /// For a device output: mute the master from its first sample past
    /// [`MASTER_GUARD_DBFS`] until playback starts again (ADR 0282). Offline
    /// rendering leaves it off, so exported values never change.
    pub fn guard_output(mut self) -> Self {
        self.guard_output = true;
        self
    }

    /// Set by the caller to stop starting helpers, for example when the output
    /// this prepares for closes. Preparing then fails with
    /// [`PlaybackError::Cancelled`] (ADR 0334).
    pub fn cancel(mut self, flag: &'a std::sync::atomic::AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }
}

/// Prepares both sides of a transport. Nothing is shared with a device until
/// the caller hands over the returned audio side.
pub fn playback(
    project: &Project,
    rate: u32,
    frames: usize,
    options: PlaybackOptions<'_>,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    let PlaybackOptions {
        clock,
        plugins,
        workgroup,
        guard_output,
        cancel,
    } = options;
    if clock.position() < 0 {
        return Err(PlaybackError::Time);
    }
    let (mut control, mut audio) = transport_pair(project, rate, frames, clock)?;
    audio.guard_output = guard_output;
    control.workgroup = workgroup;
    if let Some(PluginSetup { host_bin, routes }) = plugins {
        let chain = chain_of(project, &audio.current);
        control.hosts = HostGraph::open(
            host_bin,
            routes,
            &chain,
            frames,
            rate,
            control.workgroup.as_ref(),
            cancel,
        )?;
        control.save_controls = control.hosts.save_controls();
        let (delays, compensated) = compensate_plan(&mut audio.current, &control.hosts, frames)?;
        audio.hosts = Box::new(control.hosts.audio_snapshot(delays));
        audio
            .status
            .latency
            .store(compensated.latency, Ordering::Release);
        control.compensated = compensated;
        control.chain = chain;
        control.routes = routes.to_vec();
        control.host_bin = host_bin.to_path_buf();
    }
    Ok((control, audio))
}

fn transport_pair(
    project: &Project,
    rate: u32,
    frames: usize,
    clock: Box<dyn ClockSource>,
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    // Queue depths limit pending replacements/commands, never tracks or notes.
    let (plans, incoming) = RingBuffer::new(2);
    let (returning, returns) = RingBuffer::new(2);
    let (commands, actions) = RingBuffer::new(32);
    let (preview_returning, preview_returns) = RingBuffer::new(32);
    let (notes, audio_log) = crate::audio_log::note_pair();
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
        chase_from: AtomicI64::new(i64::MIN),
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
        master_muted: AtomicBool::new(false),
        master_mute_peak: AtomicU32::new(0),
        master_mute_at: AtomicI64::new(0),
        latency: AtomicU64::new(0),
    });
    let control = PlaybackControl {
        workgroup: None,
        plans,
        returns,
        commands,
        preview_returns,
        command_revision: 0,
        held: None,
        hosts: HostGraph::empty(),
        background: false,
        preparing: None,
        preparation_error: None,
        background_media: false,
        media: None,
        media_inputs: None,
        media_restore_states: false,
        media_plan: None,
        replacements: 0,
        published_plan: 0,
        published_tempo: current.tempo.clone(),
        chain: Vec::new(),
        previous_chases: current
            .demands
            .iter()
            .map(|demand| (demand.key, demand.chase))
            .collect(),
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
        compensated: Compensated::default(),
        compensation_changes: 0,
        audio_log,
    };
    let audio = PlaybackAudio {
        plans: incoming,
        returns: returning,
        commands: actions,
        preview_returns: preview_returning,
        applied_command: 0,
        reset_plugins: false,
        output_revision: 0,
        live_output: false,
        live_processed: false,
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
        loop_armed: false,
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
        guard_output: false,
        master_mute: None,
        notes,
    };
    Ok((control, audio))
}

impl PlaybackControl {
    /// Prepares replacement plugin graphs away from the caller. Initial output
    /// preparation and offline rendering keep their synchronous path.
    pub fn prepare_in_background(&mut self) {
        self.background = true;
    }

    /// While `enabled`, a replacement whose media is not loaded yet loads it on
    /// another thread and publishes once it arrives (ADR 0147). A load in
    /// progress takes later replacements with it either way.
    pub fn load_media_in_background(&mut self, enabled: bool) {
        self.background_media = enabled;
    }

    /// Media for the newest replacement is loading on another thread, or the
    /// plan carrying it has not been published yet.
    pub fn loading_media(&self) -> bool {
        self.media.is_some()
            || self.media_plan.is_some_and(|revision| {
                // Sequence arithmetic, as for the transport's revisions.
                self.published_plan.wrapping_sub(revision) > u64::MAX / 2
            })
    }

    /// A replacement's media or helper catalog is being prepared, or waiting to publish.
    pub fn preparing_graph(&self) -> bool {
        self.preparing.is_some() || self.held.is_some() || self.media.is_some()
    }

    /// Stops starting helpers for the replacement being prepared, so dropping
    /// or closing need not wait for them. That replacement then fails with
    /// [`PlaybackError::Cancelled`]; the published graph keeps sounding (ADR 0334).
    pub fn cancel_preparation(&self) {
        if let Some(job) = &self.preparing {
            job.cancel();
        }
    }

    /// The newest plan requested so far, published or not.
    pub fn requested_plan(&self) -> u64 {
        self.status.requested_plan.load(Ordering::Acquire)
    }

    /// `revision`, or a plan requested after it, has reached the callback's
    /// queue. A preparation that fails reports its error instead.
    pub fn published(&self, revision: u64) -> bool {
        // Sequence arithmetic, as for the transport's revisions.
        self.published_plan.wrapping_sub(revision) <= u64::MAX / 2
    }

    /// Declared device tails summed along the longest path to master: a
    /// child's tail continues through its parents' devices (ADR 0339).
    /// `u32::MAX` is infinite. From the most recently published graph.
    pub fn tail_samples(&self) -> u32 {
        self.compensated.tail
    }

    /// How far master lags the timeline in the most recently published plan,
    /// in device frames. An export drops this many frames first (ADR 0339).
    pub fn latency_samples(&self) -> u64 {
        self.compensated.latency
    }

    /// How many published plans were compensated from other latencies than
    /// the plan before them. A render that must stay aligned from its first
    /// frame to its last starts over when this moves (ADR 0340).
    pub fn compensation_changes(&self) -> u64 {
        self.compensation_changes
    }

    /// Every helper still reports the latency the published plan compensates,
    /// and no other plan is on its way. While this is false, a path is heard
    /// at another time than the plan aligned it to (ADR 0340).
    pub fn latency_settled(&self) -> bool {
        self.held.is_none()
            && self.preparing.is_none()
            && self.media.is_none()
            && !self.hosts.latency_differs(&self.compensated.reported)
    }

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
            return Err(PlaybackError::TransportBusy);
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
        let command = match command {
            PlaybackCommand::PreviewFader { track, gain, pan } => {
                return self.preview_faders(&[(track, gain, pan)]);
            }
            PlaybackCommand::SetLoop { start, end } if end > start => {
                return self.set_loop(Some((start, end)));
            }
            PlaybackCommand::SetLoop { .. } | PlaybackCommand::ClearLoop => {
                return self.set_loop(None);
            }
            PlaybackCommand::Play => QueuedCommand::Play,
            PlaybackCommand::Pause => QueuedCommand::Pause,
            PlaybackCommand::Stop => QueuedCommand::Stop,
            PlaybackCommand::Locate(sample) => QueuedCommand::Locate(sample),
            PlaybackCommand::Rewind(sample) => QueuedCommand::Rewind(sample),
            PlaybackCommand::LocateTick(tick) => QueuedCommand::LocateTick(tick),
            PlaybackCommand::RewindTick(tick) => QueuedCommand::RewindTick(tick),
            PlaybackCommand::SetMetronome(enabled) => QueuedCommand::SetMetronome(enabled),
            PlaybackCommand::ClearFaderPreview { after_plan } => {
                QueuedCommand::ClearFaderPreview { after_plan }
            }
        };
        let revision = self.command_revision.wrapping_add(1);
        self.commands
            .push((revision, command))
            .map_err(|_| PlaybackError::BufferTooSmall)?;
        self.command_revision = revision;
        Ok(())
    }

    /// Receipt for the most recently queued transport command.
    pub fn command_revision(&self) -> u64 {
        self.command_revision
    }

    fn set_loop(&mut self, bounds: Option<(i64, i64)>) -> Result<(), PlaybackError> {
        let previous = std::mem::replace(&mut self.loop_bounds, bounds);
        let result = self.reprepare(false);
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

    /// Largest shared input-event bank on each live plugin track. Banks only grow.
    pub fn plugin_event_caps(&self) -> Vec<usize> {
        self.preparing.as_ref().map_or_else(
            || self.hosts.track_event_caps(),
            |job| job.event_caps.clone(),
        )
    }

    pub fn preview_fader(&mut self, track: u128, gain: f32, pan: f32) -> Result<(), PlaybackError> {
        self.command(PlaybackCommand::PreviewFader { track, gain, pan })
    }

    /// Publishes a complete transient group in one callback command. Queue depth
    /// bounds pending gestures, never the number of affected tracks.
    pub fn preview_faders(&mut self, values: &[(u128, f32, f32)]) -> Result<(), PlaybackError> {
        let tracks: std::collections::BTreeSet<_> = self
            .project
            .tracks()
            .iter()
            .map(|track| track.id.raw())
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        if values.is_empty()
            || values.iter().any(|&(track, gain, pan)| {
                !tracks.contains(&track)
                    || !seen.insert(track)
                    || !gain.is_finite()
                    || !(0.0..=4.0).contains(&gain)
                    || !pan.is_finite()
                    || !(-1.0..=1.0).contains(&pan)
            })
        {
            return Err(PlaybackError::InvalidFader);
        }
        while self.preview_returns.pop().is_ok() {}
        let revision = self.command_revision.wrapping_add(1);
        self.commands
            .push((revision, QueuedCommand::Faders(values.into())))
            .map_err(|_| PlaybackError::BufferTooSmall)?;
        self.command_revision = revision;
        Ok(())
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
        self.replacements = self.replacements.wrapping_add(1);
        let result = self.reprepare(true);
        if result.is_err() {
            self.project = previous;
        }
        result
    }

    fn reprepare(&mut self, restore_states: bool) -> Result<(), PlaybackError> {
        self.service_queues();
        if let Some(error) = self.take_preparation_error() {
            return Err(error);
        }
        let inputs = MediaInputs::of(&self.project);
        // Calling the loader beside a load in progress would wait on its files.
        if self.media.is_some()
            || (self.background_media && self.media_inputs.as_ref() != Some(&inputs))
        {
            return self.defer_media(inputs, restore_states);
        }
        let loaded = (self.clip_loader)(&self.project, self.rate)?;
        self.media_inputs = Some(inputs);
        self.prepare_plan(&loaded, restore_states)
    }

    fn defer_media(
        &mut self,
        inputs: MediaInputs,
        restore_states: bool,
    ) -> Result<(), PlaybackError> {
        if self.media.is_none() {
            self.media = Some(MediaPreparation::begin(
                Arc::clone(&self.clip_loader),
                self.project.clone(),
                inputs,
                self.rate,
                self.replacements,
            )?);
        }
        self.media_restore_states |= restore_states;
        // Reported as a pending plan. A fader preview this edit commits holds
        // until the plan that carries its media.
        self.status.requested_plan.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Prepares the newest project once the media it needs has loaded.
    fn adopt_media(&mut self) -> Result<(), PlaybackError> {
        let Some(result) = self.media.as_mut().and_then(MediaPreparation::poll) else {
            return Ok(());
        };
        self.media = None;
        let restore_states = std::mem::take(&mut self.media_restore_states);
        let adopted = result.and_then(|loaded| {
            let clips = loaded.clips?;
            self.media_inputs = Some(loaded.inputs);
            let inputs = MediaInputs::of(&self.project);
            if self.media_inputs.as_ref() != Some(&inputs) {
                // Edited again meanwhile. The next load reuses this one's planes.
                return self.defer_media(inputs, restore_states);
            }
            let clips = if loaded.replacement == self.replacements {
                clips
            } else {
                // Other edits came meanwhile; the same media is the loader's to reuse.
                (self.clip_loader)(&self.project, self.rate)?
            };
            self.prepare_plan(&clips, restore_states)?;
            self.media_plan = Some(self.status.requested_plan.load(Ordering::Acquire));
            Ok(())
        });
        if adopted.is_err() {
            self.status
                .requested_plan
                .store(self.published_plan, Ordering::Release);
        }
        adopted
    }

    fn prepare_plan(
        &mut self,
        loaded: &[LoadedClip],
        restore_states: bool,
    ) -> Result<(), PlaybackError> {
        let prepared = Prepared::new(
            &self.project,
            self.rate,
            self.frames,
            &mut self.ids,
            PreparedSources {
                clips: loaded,
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
        // Coalescing a loop change must not discard a pending project edit.
        let restore_states = restore_states
            || self
                .held
                .as_ref()
                .is_some_and(|pending| pending.restore_states)
            || self
                .preparing
                .as_ref()
                .is_some_and(|job| job.restore_states);
        self.held = Some(Pending {
            prepared: Box::new(prepared),
            chain,
            routes: self.routes.clone(),
            revision,
            restore_states,
        });
        let result = self.publish_pending();
        if result.is_err() {
            self.reserved_events = previous_events;
        }
        result
    }

    fn publish_pending(&mut self) -> Result<(), PlaybackError> {
        if let Some(job) = &mut self.preparing {
            let Some(result) = job.poll() else {
                return Ok(());
            };
            self.preparing = None;
            let (hosts, pending, result) = result?;
            self.hosts = hosts;
            if let Err(error) = result {
                self.held = None;
                self.status
                    .requested_plan
                    .store(self.published_plan, Ordering::Release);
                return Err(error);
            }
            self.publish_graph(pending)?;
        }
        if self.plans.slots() == 0 {
            return Ok(());
        }
        let Some(mut pending) = self.held.take() else {
            return Ok(());
        };
        pending
            .prepared
            .reserve_previous_releases(&self.previous_chases)?;
        let demands: BTreeMap<_, _> = pending
            .prepared
            .demands
            .iter()
            .map(|demand| (demand.key, demand.events))
            .collect();
        for chain in &mut pending.chain {
            if let Some(&events) = demands.get(&chain.key) {
                chain.events = events;
            }
        }
        self.reserved_events = pending.prepared.events.len();
        if !self.hosts.can_reconcile(&pending.chain) {
            self.held = Some(pending);
            return Ok(());
        }
        if self.background && !self.host_bin.as_os_str().is_empty() {
            let hosts = std::mem::replace(&mut self.hosts, HostGraph::empty());
            self.preparing = Some(GraphPreparation::begin(
                hosts,
                pending,
                self.host_bin.clone(),
                self.frames,
                self.rate,
                self.workgroup.clone(),
            )?);
            return Ok(());
        }
        if !self.host_bin.as_os_str().is_empty() {
            self.hosts.take_restarts();
            let checkpoint = self.hosts.preparation_checkpoint();
            if let Err(error) = self.hosts.reconcile(
                &self.host_bin,
                crate::plugins::ReconcileRoutes {
                    entries: &pending.routes,
                    restore_states: pending.restore_states,
                    preserve: &[],
                    cancel: None,
                },
                &pending.chain,
                self.frames,
                self.rate,
                self.workgroup.as_ref(),
            ) {
                self.status
                    .requested_plan
                    .store(self.published_plan, Ordering::Release);
                self.hosts.cancel_preparation(&checkpoint)?;
                return Err(error);
            }
            self.save_controls = self.hosts.save_controls();
        }
        self.publish_graph(pending)
    }

    fn publish_graph(&mut self, mut pending: Pending) -> Result<(), PlaybackError> {
        self.save_controls = self.hosts.save_controls();
        let (delays, compensated) =
            match compensate_plan(&mut pending.prepared, &self.hosts, self.frames) {
                Ok(planned) => planned,
                Err(error) => {
                    self.status
                        .requested_plan
                        .store(self.published_plan, Ordering::Release);
                    return Err(error);
                }
            };
        // Place the cursor where the callback last stood, so adopting it only
        // advances over the notes since (R1). The callback checks the rule.
        let sample = self.status.sample.load(Ordering::SeqCst);
        let from =
            Some(self.status.chase_from.load(Ordering::SeqCst)).filter(|from| *from != i64::MIN);
        let Prepared { plan, cursor, .. } = &mut *pending.prepared;
        cursor.catch_up_from(plan, sample, from);
        self.previous_chases = pending
            .prepared
            .demands
            .iter()
            .map(|demand| (demand.key, demand.chase))
            .collect();
        self.chain = pending.chain;
        let retime = pending.prepared.tempo != self.published_tempo;
        if retime {
            self.published_tempo = pending.prepared.tempo.clone();
        }
        let update = Update {
            prepared: pending.prepared,
            hosts: Box::new(self.hosts.audio_snapshot(delays)),
            revision: pending.revision,
            retime,
        };
        // Only this control thread produces updates; the reserved slot remains free.
        self.plans
            .push(update)
            .map_err(|_| PlaybackError::BufferTooSmall)?;
        self.published_plan = pending.revision;
        if compensated.reported != self.compensated.reported
            || compensated.latency != self.compensated.latency
        {
            self.compensation_changes = self.compensation_changes.wrapping_add(1);
        }
        self.compensated = compensated;
        Ok(())
    }

    pub fn service(&mut self) {
        self.service_queues();
        self.follow_latency();
    }

    /// A helper that restarts may report another latency. The plan is
    /// prepared again, as for a loop change, so every path stays aligned
    /// (ADR 0339). A plan already on its way plans from the new value.
    fn follow_latency(&mut self) {
        if self.held.is_some()
            || self.preparing.is_some()
            || self.media.is_some()
            || self.preparation_error.is_some()
            || !self.hosts.latency_differs(&self.compensated.reported)
        {
            return;
        }
        if let Err(error) = self.reprepare(false) {
            self.preparation_error = Some(error);
        }
    }

    fn service_queues(&mut self) {
        while self.returns.pop().is_ok() {}
        while self.preview_returns.pop().is_ok() {}
        self.log_audio(false);
        for (device, control) in &mut self.save_controls {
            if let Some(bytes) = control.poll_state_save() {
                self.plugin_saves.push((*device, bytes));
            }
        }
        if let Err(error) = self.adopt_media().and_then(|()| self.publish_pending()) {
            self.preparation_error = Some(error);
        }
    }

    /// Logs what the audio thread noted since the last call, naming each node
    /// from this thread's project. `closing` also logs every pending count.
    fn log_audio(&mut self, closing: bool) {
        let hosts = &self.hosts;
        let exited = |device| hosts.exited(device);
        let master = self.status.master_muted.load(Ordering::SeqCst).then(|| {
            (
                f32::from_bits(self.status.master_mute_peak.load(Ordering::SeqCst)),
                self.status.master_mute_at.load(Ordering::SeqCst),
            )
        });
        let scene = Scene {
            project: &self.project,
            meters: &self.meters,
            master,
            exited: &exited,
        };
        if closing {
            self.audio_log.finish(&scene);
        } else {
            self.audio_log.drain(Instant::now(), &scene);
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

    /// Window notices and changes each running plugin made on its own since
    /// the last call. Reading them never waits for a helper's socket.
    pub fn take_plugin_activity(&self) -> Vec<(u128, echode_sandbox::PluginActivity)> {
        self.save_controls
            .iter()
            .filter_map(|(device, control)| Some((*device, control.take_activity()?)))
            .collect()
    }

    /// The helper currently hosting `device`. Offline, unresolved and not yet
    /// published devices have none.
    fn device_control(
        &self,
        device: u128,
    ) -> Result<&echode_sandbox::PluginControl, PlaybackError> {
        self.save_controls
            .iter()
            .find(|(id, _)| *id == device)
            .map(|(_, control)| control)
            .ok_or(PlaybackError::NoRunningPlugin)
    }

    /// A control-only endpoint, so another thread can wait for a device reply.
    /// Never call it from an audio callback.
    pub fn device_control_handle(
        &self,
        device: u128,
    ) -> Result<echode_sandbox::PluginControl, PlaybackError> {
        self.device_control(device).cloned()
    }

    /// What the running helper last saved or loaded, sharing its bytes.
    /// `Ok(None)` is a helper still on the plugin's defaults; `Err` is a
    /// device without a helper.
    pub fn device_applied_state(
        &self,
        device: u128,
    ) -> Result<Option<echode_sandbox::AppliedState>, PlaybackError> {
        Ok(self.device_control(device)?.applied_state())
    }

    /// Loads recorded bytes into the running helper without replacing it.
    pub fn restore_device_state(&self, device: u128, bytes: &[u8]) -> Result<(), PlaybackError> {
        self.device_control(device)?
            .restore_state(bytes)
            .map_err(|error| PlaybackError::helper(device, error))
    }

    /// Brings the running helpers of `devices` to their recorded states, for
    /// an Undo, Redo or API edit that changed only plugin states. `recorded`
    /// gives a device's state, `Ok(None)` for plugin defaults. A helper that
    /// already holds that state, by name, is left alone and nothing is read.
    /// Returns `true` when the caller must replace the graph: defaults need a
    /// fresh plugin, and an unreadable or refused load needs a helper spawned
    /// with the state. A device without a helper is left to that replacement.
    pub fn restore_recorded_states<E>(
        &self,
        devices: &[u128],
        recorded: impl Fn(u128) -> Result<Option<crate::PluginState>, E>,
    ) -> bool {
        let mut replace = false;
        for &device in devices {
            let Ok(control) = self.device_control(device) else {
                continue;
            };
            let applied = control.applied_state();
            match recorded(device) {
                Ok(want) if crate::plugin_state::holds(applied.as_ref(), want.as_ref()) => {}
                Ok(Some(want)) => {
                    replace |= want
                        .load()
                        .ok()
                        .is_none_or(|state| control.restore_applied(&state).is_err());
                }
                Ok(None) | Err(_) => replace = true,
            }
        }
        replace
    }
    /// Device id and reason when a plugin missed the block that just rendered.
    /// Live playback still silences that block and leaves [`PlaybackStatus::failed`]
    /// clear. Export uses this so a deadline miss is not a finished file.
    pub fn plugin_failure(&self) -> Option<crate::PluginBlockFailure> {
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
                master_auto_mute: self.status.master_muted.load(Ordering::SeqCst).then(|| {
                    MasterAutoMute {
                        peak: f32::from_bits(self.status.master_mute_peak.load(Ordering::SeqCst)),
                        at: self.status.master_mute_at.load(Ordering::SeqCst),
                    }
                }),
                latency_samples: self.status.latency.load(Ordering::Acquire),
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
    /// The score source's fade position, before plugins, not a master mute.
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

    /// Live stop releases score notes and fades placed audio before plugins.
    /// The clock holds while the graph keeps processing its tails and live output.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render_live_gated_until(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
    ) {
        let sample = self.clock.position();
        let _ = self.render_mode(left, right, deadline, enabled, true);
        // The engine's own deadline (ADR 0283): a later return is logged.
        let late = Instant::now().saturating_duration_since(deadline);
        if !late.is_zero() {
            self.notes.send(Note::Late {
                sample,
                late_ns: u64::try_from(late.as_nanos()).unwrap_or(u64::MAX),
                frames: u32::try_from(left.len()).unwrap_or(u32::MAX),
            });
        }
    }

    /// A start cancelled before any device block was heard cannot emit a tail.
    pub fn cancel_unheard_start(&mut self, sample: i64) {
        // A stop that already reached this block returned the clock itself,
        // possibly to the remembered start; keep that position.
        if self.playing {
            self.clock.locate(sample);
            self.arm_loop();
        }
        self.playing = false;
        self.stop_tail = None;
        self.stop_fade_offset = None;
        self.live_output = false;
        for track in &self.current.tracks {
            track.meter.clear();
        }
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
        self.current.meters_committed = false;
        let start = self.clock.position();
        let result = self.step(left, right, deadline, enabled, live, release);
        if self.guard_output {
            self.guard_master(left, right, start);
        }
        self.live_processed |= live && self.current.meters_committed;
        if !self.current.meters_committed {
            for track in &self.current.tracks {
                track.meter.publish(0.0, release);
            }
        }
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
        self.status.chase_from.store(
            self.current.cursor.chase_from().unwrap_or(i64::MIN),
            Ordering::SeqCst,
        );
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
        self.status
            .master_muted
            .store(self.master_mute.is_some(), Ordering::SeqCst);
        if let Some(mute) = self.master_mute {
            self.status
                .master_mute_peak
                .store(mute.peak.to_bits(), Ordering::SeqCst);
            self.status.master_mute_at.store(mute.at, Ordering::SeqCst);
        }
        self.status.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn restore_fader_preview(&mut self, live: bool) {
        if let Some(previous) = self.fader_preview.take() {
            self.current.preview_faders(
                &previous,
                true,
                (self.playing && (self.live_output || !live)) || (live && self.live_processed),
            );
            // The caller reserves a return slot before taking ownership.
            if let Err(rtrb::PushError::Full(_)) = self.preview_returns.push(previous) {
                std::process::abort();
            }
        }
    }

    fn step(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        deadline: Instant,
        enabled: bool,
        live: bool,
        meter_release: f32,
    ) -> Result<(), PlaybackError> {
        if left.len() != right.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
        self.stop_fade_offset = None;
        let mut faders_changed = false;
        if self.stop_tail.is_none()
            && !self.reset_plugins
            && !self.hosts.cleanup_pending()
            && self.returns.slots() > 0
            && let Ok(mut next) = self.plans.pop()
        {
            let position = self.clock.position();
            let carried = next.prepared.carry(
                &mut self.current,
                position,
                self.playing || (live && self.live_processed),
            );
            // The clock's tick under the replaced plan's tempo, read before
            // that plan returns to the control thread.
            let mut retimed = None;
            if carried.is_ok() {
                if next.retime {
                    retimed = Some(
                        self.current
                            .tempo
                            .sample_to_tick(position, self.current.sample_rate)
                            .ok_or(PlaybackError::Time),
                    );
                }
                next.hosts.carry(&mut self.hosts);
                next.hosts.activate_prepared();
                std::mem::swap(&mut next.prepared, &mut self.current);
                std::mem::swap(&mut next.hosts, &mut self.hosts);
                self.current.meters_committed = false;
                self.status
                    .latency
                    .store(self.current.latency, Ordering::Release);
                faders_changed = true;
                self.status
                    .applied_plan
                    .store(next.revision, Ordering::Release);
            }
            if let Err(rtrb::PushError::Full(_)) = self.returns.push(next) {
                std::process::abort();
            }
            carried?;
            if let Some(tick) = retimed {
                // A tempo edit keeps the beat that was sounding (ADR 0330).
                let sample = self.current.sample_at(tick?)?;
                if sample != position {
                    self.locate(sample)?;
                }
            }
        }
        if (self.loop_start, self.loop_end) != (self.current.loop_start, self.current.loop_end) {
            self.loop_start = self.current.loop_start;
            self.loop_end = self.current.loop_end;
            self.arm_loop();
        }
        while self.preview_returns.slots() > 0 {
            let Ok((revision, command)) = self.commands.pop() else {
                break;
            };
            match command {
                QueuedCommand::Play => {
                    self.stop_tail = None;
                    self.output_revision = self.output_revision.wrapping_add(1);
                    self.playing = true;
                    self.failed = false;
                    self.release_auto_mutes();
                    self.current.place(self.clock.position(), false)?;
                }
                QueuedCommand::Pause | QueuedCommand::Stop => {
                    if !live {
                        self.output_revision = self.output_revision.wrapping_add(1);
                    }
                    self.begin_stop(live)?;
                }
                QueuedCommand::Rewind(sample) => self.rewind(sample, live)?,
                QueuedCommand::RewindTick(tick) => {
                    let sample = self.current.sample_at(tick)?;
                    self.rewind(sample, live)?;
                }
                QueuedCommand::Locate(sample) => self.locate(sample)?,
                QueuedCommand::LocateTick(tick) => {
                    let sample = self.current.sample_at(tick)?;
                    self.locate(sample)?;
                }
                QueuedCommand::SetMetronome(enabled) => {
                    self.metronome = enabled;
                    if !enabled {
                        // Clicks still inside the compensation are not heard later.
                        self.current.clicks_delay.clear();
                    }
                }
                QueuedCommand::ClearFaderPreview { after_plan } => {
                    self.clear_preview_after = Some(after_plan)
                }
                QueuedCommand::Faders(values) => {
                    self.restore_fader_preview(live);
                    self.fader_preview = Some(values);
                    self.clear_preview_after = None;
                    faders_changed = true;
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
        }) && self.preview_returns.slots() > 0
        {
            self.restore_fader_preview(live);
            self.clear_preview_after = None;
            faders_changed = true;
        }
        if faders_changed {
            self.current.preview_faders(
                self.fader_preview.as_deref().unwrap_or_default(),
                false,
                (self.playing && (self.live_output || !live)) || (live && self.live_processed),
            );
        }
        if !enabled && self.playing {
            if !live {
                self.output_revision = self.output_revision.wrapping_add(1);
            }
            self.begin_stop(live)?;
        }
        if let Some(tail) = self.stop_tail {
            self.render_stop_tail(tail, left, right, deadline, meter_release)?;
            return Ok(());
        }
        if self.reset_plugins {
            // The compensation's lines hold audio from before the jump, as the
            // plugins did; both start afresh (ADR 0339).
            self.current.clear_delays();
            self.hosts.clear_delays();
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
            self.count_misses();
            if !completed {
                return Ok(());
            }
            if self.playing {
                self.output_revision = self.output_revision.wrapping_add(1);
            }
        }
        let loop_bounds = self.active_loop();
        if !self.playing {
            if live {
                self.current.render(
                    &[],
                    left,
                    right,
                    &mut self.hosts,
                    deadline,
                    RenderRange {
                        loop_bounds,
                        started_before: Some(self.clock.position()),
                        source_fade: None,
                        meter_release,
                    },
                )?;
                self.count_misses();
                if self.metronome {
                    flush_clicks(self, left, right)?;
                }
            } else {
                let flushed = self.current.released != 0;
                self.current.flush_releases(&mut self.hosts, deadline)?;
                if flushed {
                    self.count_misses();
                }
            }
            return Ok(());
        }
        let (loop_start, loop_end) = loop_bounds;
        let (span_count, end) = split_loop(
            self.clock.position(),
            left.len(),
            loop_start,
            loop_end,
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
                loop_bounds,
                started_before: None,
                source_fade: None,
                meter_release,
            },
        )?;
        self.count_misses();
        if self.metronome {
            mix_metronome(self, span_count, left, right, None)?;
        }
        self.clock.locate(end);
        self.live_output |= live;
        Ok(())
    }

    /// Starting playback is how the user asks to hear again (ADR 0282). The
    /// plugins of an auto-muted track start afresh, so the state that faulted
    /// does not fault again at once.
    /// Counts a block in which a plugin missed, and notes each miss for the log.
    fn count_misses(&mut self) {
        if !self.hosts.missed_block() {
            return;
        }
        self.status
            .plugin_missed_blocks
            .fetch_add(1, Ordering::Relaxed);
        let sample = self.clock.position();
        for &(device, reason) in self.hosts.misses() {
            self.notes.send(Note::Plugin {
                device,
                reason,
                sample,
            });
        }
    }

    fn release_auto_mutes(&mut self) {
        for track in &self.current.tracks {
            if track.meter.auto_muted() {
                track.meter.clear_auto_mute();
                self.hosts.reset_track(track.key);
            }
        }
        self.master_mute = None;
    }

    /// Mutes the device output from its first sample past the guard, or a NaN,
    /// and keeps it muted (ADR 0282). The values are not limited.
    fn guard_master(&mut self, left: &mut [f32], right: &mut [f32], start: i64) {
        let from = if self.master_mute.is_some() {
            0
        } else {
            let Some((first, peak)) =
                left.iter()
                    .zip(right.iter())
                    .enumerate()
                    .find_map(|(frame, (left, right))| {
                        // `f32::max` drops a NaN, so test each channel for one first.
                        let peak = [*left, *right]
                            .into_iter()
                            .find(|sample| sample.is_nan())
                            .unwrap_or_else(|| left.abs().max(right.abs()));
                        (peak.is_nan() || peak > MASTER_GUARD).then_some((frame, peak))
                    })
            else {
                return;
            };
            self.master_mute = Some(MasterAutoMute {
                peak,
                at: start.saturating_add(first as i64),
            });
            first
        };
        for plane in [left, right] {
            if let Some(rest) = plane.get_mut(from..) {
                rest.fill(0.0);
            }
        }
    }

    fn arm_loop(&mut self) {
        self.loop_armed = self.loop_end.is_some_and(|end| self.clock.position() < end);
    }

    fn active_loop(&self) -> (Option<i64>, Option<i64>) {
        if self.loop_armed {
            (self.loop_start, self.loop_end)
        } else {
            (None, None)
        }
    }

    fn locate(&mut self, sample: i64) -> Result<(), PlaybackError> {
        if self.stop_tail.is_some() {
            self.finish_stop()?;
        }
        self.output_revision = self.output_revision.wrapping_add(1);
        self.clock.locate(sample);
        self.arm_loop();
        self.current.place(sample, true)?;
        self.reset_plugins = true;
        Ok(())
    }

    fn rewind(&mut self, sample: i64, live: bool) -> Result<(), PlaybackError> {
        if self.playing {
            if !live {
                self.output_revision = self.output_revision.wrapping_add(1);
            }
            self.begin_stop(live)?;
        }
        // A stop tail renders from its own position, so moving
        // the clock does not cut it or reset any plugin.
        self.clock.locate(sample);
        self.arm_loop();
        self.current.place(sample, true)?;
        Ok(())
    }

    fn begin_stop(&mut self, live: bool) -> Result<(), PlaybackError> {
        if live
            && self.live_output
            && self.playing
            && self.stop_tail.is_none()
            && !self.reset_plugins
            && !self.hosts.cleanup_pending()
        {
            self.stop_tail = Some(StopTail {
                position: self.clock.position(),
                cutoff: self.clock.position(),
                remaining: (self.current.sample_rate / 200).max(2),
                loop_bounds: self.active_loop(),
            });
        }
        if live {
            if self.playing {
                self.current.place(self.clock.position(), true)?;
            }
            self.playing = false;
            self.live_output = false;
            return Ok(());
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
        meter_release: f32,
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
            left,
            right,
            &mut self.hosts,
            deadline,
            RenderRange {
                loop_bounds: tail.loop_bounds,
                started_before: Some(tail.cutoff),
                source_fade: self.stop_fade_offset,
                meter_release,
            },
        )?;
        if self.metronome {
            *self
                .spans
                .first_mut()
                .ok_or(PlaybackError::BufferTooSmall)? = span;
            mix_metronome(self, 1, left, right, Some(tail.cutoff))?;
        }
        self.count_misses();
        tail.remaining -= frames as u32;
        tail.position = tail
            .position
            .checked_add(frames as i64)
            .ok_or(PlaybackError::Time)?;
        if tail.remaining == 0 {
            self.stop_tail = None;
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
    let (loop_start, loop_end) = audio.active_loop();
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
            loop_end,
            wrap_from: span.release_at.and(loop_start),
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
        .get_mut(..left.len())
        .ok_or(PlaybackError::BufferTooSmall)?;
    if let (Some(_), Some(offset)) = (cutoff, audio.stop_fade_offset) {
        for (index, click) in clicks.iter_mut().enumerate() {
            *click *= crate::declick::stop_gain(
                u64::from(offset) + index as u64,
                (audio.current.sample_rate / 200).max(2),
            );
        }
    }
    // With the music, which master hears after the compensation (ADR 0339).
    audio.current.clicks_delay.process(clicks, None);
    for ((sample_left, sample_right), click) in left.iter_mut().zip(right.iter_mut()).zip(&*clicks)
    {
        *sample_left += *click;
        *sample_right += *click;
    }
    Ok(())
}

/// While stopped, clicks still inside the compensation play out, as the
/// music's delayed tail does.
fn flush_clicks(
    audio: &mut PlaybackAudio,
    left: &mut [f32],
    right: &mut [f32],
) -> Result<(), PlaybackError> {
    if audio.current.clicks_delay.frames() == 0 {
        return Ok(());
    }
    let clicks = audio
        .clicks
        .get_mut(..left.len())
        .ok_or(PlaybackError::BufferTooSmall)?;
    clicks.fill(0.0);
    audio.current.clicks_delay.process(clicks, None);
    for ((sample_left, sample_right), click) in left.iter_mut().zip(right.iter_mut()).zip(&*clicks)
    {
        *sample_left += *click;
        *sample_right += *click;
    }
    Ok(())
}

#[cfg(test)]
#[path = "playback_tests.rs"]
mod tests;
