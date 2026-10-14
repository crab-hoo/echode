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

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, Ordering};

use echode_model::{Meter, Project};
use echode_realtime::{Cursor, Event, Plan, Span, TempoIndex, sort_events};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::plugins::{ChainDevice, HostGraph, PluginRoute, TrackChain, block_deadline};
use crate::realtime::ForbidAlloc;
use crate::sequence::render_metronome_index;
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
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub sample: i64,
    pub tick: i64,
    pub failed: bool,
}

struct Status {
    generation: AtomicU32,
    playing: AtomicBool,
    sample: AtomicI64,
    tick: AtomicI64,
    failed: AtomicBool,
}

struct Track {
    key: u32,
    synth: TestSynth,
    plane: Vec<f32>,
    /// Placed audio for this block, kept apart from the synth so an instrument
    /// can replace the synth without eating the clips.
    clip_plane: Vec<f32>,
    fader: TrackFader,
    /// Playback ids sorted for bounded lookup when carrying a live voice.
    notes: Vec<(u32, i64, i64)>,
    clips: Vec<StoredClip>,
}

struct StoredClip {
    start_sample: i64,
    samples: Arc<[f32]>,
    fade_in: u32,
    fade_out: u32,
}

/// One placement's samples at the device rate, ready for the control thread to store.
#[derive(Clone, Debug)]
pub struct LoadedClip {
    pub track: u128,
    pub start_sample: i64,
    pub samples: Arc<[f32]>,
    pub fade_in: u32,
    pub fade_out: u32,
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
    events: Vec<Event>,
    filtered: Vec<Event>,
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
}

impl Prepared {
    fn new(
        project: &Project,
        rate: u32,
        frames: usize,
        ids: &mut PlaybackIds,
        loaded: &[LoadedClip],
        min_events: usize,
    ) -> Result<Self, PlaybackError> {
        if rate == 0 || frames == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let plan = crate::plan::build_plan(project, project.tempo(), rate, ids)?;
        let cursor = plan.cursor();
        let tempo = crate::plan::tempo_index(project.tempo())?;
        let solo = plan.tracks.iter().any(|track| track.solo != 0.0);
        let mut tracks = Vec::with_capacity(plan.tracks.len());
        let mut event_count = 1usize;
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
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                })
                .collect();
            clips.sort_by_key(|clip| clip.start_sample);
            tracks.push(Track {
                key: track.track_key,
                synth,
                plane: vec![0.0; frames],
                clip_plane: vec![0.0; frames],
                fader: TrackFader {
                    gain: track.gain,
                    pan: track.pan,
                    mute: track.mute != 0.0 || (solo && track.solo == 0.0),
                    solo: false,
                },
                notes,
                clips,
            });
            event_count = event_count.saturating_add(track.notes.len().saturating_mul(3));
        }
        // A loop can retrigger every note on every sample of this block.
        // The buffer is sized here, on the control thread, so the callback
        // does not grow it. This is the block's worst case, not a project limit.
        event_count = event_count.saturating_mul(frames.max(1));
        event_count = event_count.max(min_events).max(1);
        let mut track_indices: Vec<_> = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (track.key, index))
            .collect();
        track_indices.sort_unstable_by_key(|entry| entry.0);
        Ok(Self {
            plan,
            cursor,
            tempo,
            tracks,
            track_indices,
            events: vec![Event::note_off(0, 0, 0, 0); event_count],
            filtered: vec![Event::note_off(0, 0, 0, 0); event_count],
            released: 0,
            sample_rate: rate,
            meter: project.meter(),
            chase: true,
            plugin_holds: false,
            chased_ons: Vec::with_capacity(event_count),
        })
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

    fn carry(&mut self, previous: &mut Self, sample: i64) -> Result<(), PlaybackError> {
        for track in &mut self.tracks {
            let old_index = previous
                .track_indices
                .binary_search_by_key(&track.key, |entry| entry.0)
                .ok()
                .and_then(|index| previous.track_indices.get(index))
                .map(|entry| entry.1);
            if let Some(old) = old_index.and_then(|index| previous.tracks.get_mut(index)) {
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
    ) -> Result<(), PlaybackError> {
        hosts.begin_block();
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
            .play(&self.plan, spans, rest)
            .map_err(play_error)?;
        {
            let events = self
                .events
                .get_mut(..count)
                .ok_or(PlaybackError::BufferTooSmall)?;
            sort_events(events);
        }
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
                clip_plane,
                clips,
                ..
            } = track;
            let plane = plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            let clip_plane = clip_plane
                .get_mut(..frames)
                .ok_or(PlaybackError::BufferTooSmall)?;
            plane.fill(0.0);
            clip_plane.fill(0.0);
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
            add_stored_clips(clips, spans, clip_plane)?;
            for (sample, clip) in plane.iter_mut().zip(clip_plane.iter()) {
                *sample += *clip;
            }
            hosts.submit(*key, plane, clip_plane, filtered, &self.chased_ons);
        }
        if hosts.dropped_events() {
            return Err(PlaybackError::BufferTooSmall);
        }
        let deadline = block_deadline(left.len(), self.sample_rate);
        hosts.collect(deadline);
        for track in &mut self.tracks {
            let plane = track
                .plane
                .get_mut(..left.len())
                .ok_or(PlaybackError::BufferTooSmall)?;
            hosts.apply(track.key, plane);
            crate::mix::add_prepared_mono(plane, track.fader, left, right);
        }
        self.plugin_holds = true;
        Ok(())
    }

    /// Deliver queued note-offs without rendering. Stop and pause return before
    /// [`Self::render`], and the plugin would otherwise keep the old voices.
    fn flush_releases(&mut self, hosts: &mut HostGraph) -> Result<(), PlaybackError> {
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
            hosts.submit(track.key, plane, &[], filtered, &[]);
        }
        hosts.collect(block_deadline(frames, self.sample_rate));
        Ok(())
    }
}

fn add_stored_clips(
    clips: &[StoredClip],
    spans: &[Span],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    // Sorted by start. Placements do not overlap, so ends follow the same order.
    // The callback binary-searches the first clip that can reach the span and
    // stops at the first one that starts after it.
    for span in spans {
        let end = span
            .start
            .checked_add(i64::from(span.frames))
            .ok_or(PlaybackError::Time)?;
        let first = clips.partition_point(|clip| stored_end(clip) <= span.start);
        let Some(rest) = clips.get(first..) else {
            continue;
        };
        for clip in rest.iter().take_while(|clip| clip.start_sample < end) {
            let view = [crate::voice::FadedClip {
                start_sample: clip.start_sample,
                samples: clip.samples.as_ref(),
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
            }];
            let played = [PlayedSpan {
                start_sample: span.start,
                frames: span.frames,
                block_offset: span.block_offset,
                release_at: None,
                loop_end: None,
                wrap_from: None,
            }];
            crate::voice::render_faded_clips(&view, &played, output)?;
        }
    }
    Ok(())
}

fn stored_end(clip: &StoredClip) -> i64 {
    clip.start_sample
        .saturating_add(i64::try_from(clip.samples.len()).unwrap_or(i64::MAX))
}

fn chain_of(project: &Project, prepared: &Prepared) -> Vec<TrackChain> {
    prepared
        .plan
        .tracks
        .iter()
        .filter_map(|planned| {
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
            Some(TrackChain {
                key: planned.track_key,
                devices,
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

const HOST_IDLE: u8 = 0;
const HOST_AUDIO: u8 = 1;
const HOST_CONTROL: u8 = 2;

/// The live plugin graph. The callback and the control thread take turns.
///
/// The owner flag is the only synchronization. The callback never waits: if
/// the control thread is growing event capacity, that block keeps the current
/// plan and skips plugins. The control thread waits until the callback has
/// left the graph, then remaps.
struct HostSlot {
    graph: UnsafeCell<Box<HostGraph>>,
    owner: AtomicU8,
}

// SAFETY: `owner` is 0, 1, or 2. Only the role that stored its flag creates a
// mutable reference to `graph`, and it clears the flag before the other role
// may enter. The graph is `Send`.
unsafe impl Send for HostSlot {}
unsafe impl Sync for HostSlot {}

impl HostSlot {
    fn shared(graph: HostGraph) -> Arc<Self> {
        Arc::new(Self {
            graph: UnsafeCell::new(Box::new(graph)),
            owner: AtomicU8::new(HOST_IDLE),
        })
    }

    fn try_audio(self: &Arc<Self>) -> Option<AudioHosts<'_>> {
        self.owner
            .compare_exchange(HOST_IDLE, HOST_AUDIO, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| AudioHosts { slot: self })
    }

    fn enter_control(&self) -> ControlHosts<'_> {
        while self
            .owner
            .compare_exchange(HOST_IDLE, HOST_CONTROL, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            std::hint::spin_loop();
        }
        ControlHosts { slot: self }
    }

    /// Exclusive access. The caller holds `HOST_AUDIO` or `HOST_CONTROL`.
    #[allow(clippy::mut_from_ref)] // the owner flag, not `&mut self`, is the lock
    fn graph_box(&self) -> &mut Box<HostGraph> {
        // SAFETY: the owner flag excludes the other role for this borrow.
        unsafe { &mut *self.graph.get() }
    }
}

struct AudioHosts<'a> {
    slot: &'a HostSlot,
}

impl AudioHosts<'_> {
    fn graph_box(&mut self) -> &mut Box<HostGraph> {
        self.slot.graph_box()
    }
}

impl Drop for AudioHosts<'_> {
    fn drop(&mut self) {
        self.slot.owner.store(HOST_IDLE, Ordering::Release);
    }
}

struct ControlHosts<'a> {
    slot: &'a HostSlot,
}

impl ControlHosts<'_> {
    fn graph(&mut self) -> &mut HostGraph {
        self.slot.graph_box()
    }
}

impl Drop for ControlHosts<'_> {
    fn drop(&mut self) {
        self.slot.owner.store(HOST_IDLE, Ordering::Release);
    }
}

pub struct PlaybackControl {
    plans: Producer<Box<Prepared>>,
    returns: Consumer<Box<Prepared>>,
    commands: Producer<PlaybackCommand>,
    held: Option<Box<Prepared>>,
    host_plans: Producer<Box<HostGraph>>,
    host_returns: Consumer<Box<HostGraph>>,
    pending_hosts: Option<Box<HostGraph>>,
    host_slot: Arc<HostSlot>,
    chain: Vec<TrackChain>,
    routes: Vec<PluginRoute>,
    host_bin: std::path::PathBuf,
    ids: PlaybackIds,
    rate: u32,
    frames: usize,
    status: Arc<Status>,
    clip_loader: ClipLoader,
    /// Largest event buffer built for this session. A smaller plan still has
    /// room for note-offs of notes the plugin already heard. It does not shrink.
    event_room: usize,
}

/// Owned by exactly one callback. Move back to the control thread before Drop.
pub struct PlaybackAudio {
    plans: Consumer<Box<Prepared>>,
    returns: Producer<Box<Prepared>>,
    commands: Consumer<PlaybackCommand>,
    host_plans: Consumer<Box<HostGraph>>,
    host_returns: Producer<Box<HostGraph>>,
    /// Recycled with the ring. The callback swaps the box and never allocates one.
    host_slot: Arc<HostSlot>,
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

/// Same transport as [`playback_pair`], and a helper for each resolved CLAP
/// device. Devices whose id is not in `routes` stay on the built-in synth.
pub fn playback_with_plugins(
    project: &Project,
    rate: u32,
    frames: usize,
    host_bin: &std::path::Path,
    routes: &[PluginRoute],
) -> Result<(PlaybackControl, PlaybackAudio), PlaybackError> {
    let (mut control, audio) =
        playback_with_clock(project, rate, frames, Box::<InternalClock>::default())?;
    let chain = chain_of(project, &audio.current);
    // The callback has not started, so the control flag is free.
    {
        let mut slot = audio.host_slot.enter_control();
        *slot.graph() = HostGraph::open(
            host_bin,
            routes,
            &chain,
            frames,
            audio.current.events.len(),
            rate,
        )?;
    }
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
    let (host_plans, host_incoming) = RingBuffer::new(2);
    let (host_returning, host_returns) = RingBuffer::new(2);
    let mut ids = PlaybackIds::new();
    let mut current = Box::new(Prepared::new(project, rate, frames, &mut ids, &[], 0)?);
    current.place(clock.position(), false)?;
    let event_room = current.events.len();
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
    });
    let host_slot = HostSlot::shared(HostGraph::empty());
    let control = PlaybackControl {
        plans,
        returns,
        commands,
        held: None,
        host_plans,
        host_returns,
        pending_hosts: None,
        host_slot: Arc::clone(&host_slot),
        chain: Vec::new(),
        routes: Vec::new(),
        host_bin: std::path::PathBuf::new(),
        ids,
        rate,
        frames,
        status: status.clone(),
        clip_loader: Arc::new(empty_clips),
        event_room,
    };
    let audio = PlaybackAudio {
        plans: incoming,
        returns: returning,
        commands: actions,
        host_plans: host_incoming,
        host_returns: host_returning,
        host_slot,
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

    pub fn command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        self.commands
            .push(command)
            .map_err(|_| PlaybackError::BufferTooSmall)
    }
    pub fn replace(&mut self, project: &Project) -> Result<(), PlaybackError> {
        self.service();
        let loaded = (self.clip_loader)(project, self.rate)?;
        let prepared = Prepared::new(
            project,
            self.rate,
            self.frames,
            &mut self.ids,
            &loaded,
            self.event_room,
        )?;
        self.event_room = self.event_room.max(prepared.events.len());
        let chain = chain_of(project, &prepared);
        if !self.host_bin.as_os_str().is_empty() {
            let mut hosts = self.host_slot.enter_control();
            if chain != self.chain {
                hosts.graph().reconcile(
                    &self.host_bin,
                    &self.routes,
                    &chain,
                    self.frames,
                    prepared.events.len(),
                    self.rate,
                )?;
                self.chain = chain;
            }
            let needed = prepared.events.len();
            if hosts.graph().event_capacity() < needed {
                hosts.graph().ensure_events(needed)?;
            }
        }
        self.held = Some(Box::new(prepared));
        self.service();
        Ok(())
    }
    pub fn service(&mut self) {
        while self.returns.pop().is_ok() {}
        while self.host_returns.pop().is_ok() {}
        if let Some(plan) = self.held.take()
            && let Err(rtrb::PushError::Full(plan)) = self.plans.push(plan)
        {
            self.held = Some(plan);
        }
        if let Some(hosts) = self.pending_hosts.take()
            && let Err(rtrb::PushError::Full(hosts)) = self.host_plans.push(hosts)
        {
            self.pending_hosts = Some(hosts);
        }
    }
    /// Device id and reason when a plugin missed the block that just rendered.
    /// Live playback still silences that block and leaves [`PlaybackStatus::failed`]
    /// clear. Export uses this so a deadline miss is not a finished file.
    pub fn plugin_failure(&self) -> Option<String> {
        let mut hosts = self.host_slot.enter_control();
        hosts.graph().export_failure()
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
            };
            if before == self.status.generation.load(Ordering::SeqCst) {
                return value;
            }
        }
    }
}

impl PlaybackAudio {
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _guard = ForbidAlloc::enter();
        left.fill(0.0);
        right.fill(0.0);
        let result = self.step(left, right);
        if result.is_err() {
            self.playing = false;
            self.failed = true;
            left.fill(0.0);
            right.fill(0.0);
        }
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
        self.status.playing.store(self.playing, Ordering::SeqCst);
        self.status.failed.store(self.failed, Ordering::SeqCst);
        self.status.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn step(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<(), PlaybackError> {
        if left.len() != right.len() {
            return Err(PlaybackError::BufferTooSmall);
        }
        // Miss the lock only while the control thread is growing event capacity.
        // Leave the new plan queued so this block does not consume its note-ons.
        let mut hosts = self.host_slot.try_audio();
        if hosts.is_some() {
            if self.returns.slots() > 0
                && let Ok(mut next) = self.plans.pop()
            {
                next.carry(&mut self.current, self.clock.position())?;
                std::mem::swap(&mut next, &mut self.current);
                // Only this thread writes the ring; the reserved slot cannot disappear.
                if let Err(rtrb::PushError::Full(_)) = self.returns.push(next) {
                    std::process::abort();
                }
            }
            if self.host_returns.slots() > 0
                && let Ok(next) = self.host_plans.pop()
                && let Some(hosts) = hosts.as_mut()
            {
                let previous = std::mem::replace(hosts.graph_box(), next);
                if let Err(rtrb::PushError::Full(_)) = self.host_returns.push(previous) {
                    std::process::abort();
                }
            }
        }
        while let Ok(command) = self.commands.pop() {
            match command {
                PlaybackCommand::Play => {
                    self.playing = true;
                    self.failed = false;
                    self.current.place(self.clock.position(), false)?;
                }
                PlaybackCommand::Pause | PlaybackCommand::Stop => {
                    // Both leave the playhead in place. The next play resumes there.
                    self.playing = false;
                    self.current.place(self.clock.position(), true)?;
                }
                PlaybackCommand::Locate(sample) => {
                    self.clock.locate(sample);
                    self.current.place(sample, true)?;
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
            }
        }
        if !self.playing {
            if let Some(hosts) = hosts.as_mut() {
                self.current.flush_releases(hosts.graph_box())?;
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
        if let Some(hosts) = hosts.as_mut() {
            self.current.render(spans, left, right, hosts.graph_box())?;
        } else {
            let mut bypass = HostGraph::empty();
            self.current.render(spans, left, right, &mut bypass)?;
        }
        drop(hosts);
        if self.metronome {
            mix_metronome(self, span_count, left, right)?;
        }
        self.clock.locate(end);
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
    render_metronome_index(
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
            fade_in: 0,
            fade_out: 0,
        }
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
        {
            let _guard = ForbidAlloc::enter();
            sort_events(&mut events);
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
