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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use echode_model::{Meter, Project};
use echode_realtime::{Cursor, Event, Plan, Span, TempoIndex, sort_events};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::plugins::{HostGraph, PluginRoute, TrackChain};
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
    sample_rate: u32,
    meter: Meter,
    chase: bool,
}

impl Prepared {
    fn new(
        project: &Project,
        rate: u32,
        frames: usize,
        ids: &mut PlaybackIds,
        loaded: &[LoadedClip],
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
            sample_rate: rate,
            meter: project.meter(),
            chase: true,
        })
    }

    fn place(&mut self, sample: i64, silence: bool) {
        self.cursor.catch_up(&self.plan, sample);
        self.chase = true;
        if silence {
            for track in &mut self.tracks {
                track.synth.silence();
            }
        }
    }

    fn carry(&mut self, previous: &mut Self, sample: i64) {
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
        self.place(sample, false);
    }

    /// Drops chased note-ons whose voice was carried from the previous plan.
    ///
    /// Those events rebuild the cursor. Playing them again would restart the
    /// envelope. Note-ons from the timeline stay, including one that lands on
    /// the first frame of a block while its release tail is still sounding.
    fn drop_carried_note_ons(&mut self, count: usize) -> Result<usize, PlaybackError> {
        let mut kept = 0usize;
        for index in 0..count {
            let event = *self
                .events
                .get(index)
                .ok_or(PlaybackError::BufferTooSmall)?;
            if event.is_note_on() && self.voice_is_sounding(event) {
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
        let mut count = if self.chase {
            self.chase = false;
            self.cursor
                .chase_events(&self.plan, &mut self.events)
                .map_err(play_error)?
        } else {
            0
        };
        count = self.drop_carried_note_ons(count)?;
        let rest = self
            .events
            .get_mut(count..)
            .ok_or(PlaybackError::BufferTooSmall)?;
        count += self
            .cursor
            .play(&self.plan, spans, rest)
            .map_err(play_error)?;
        let events = self
            .events
            .get_mut(..count)
            .ok_or(PlaybackError::BufferTooSmall)?;
        sort_events(events);
        for track in &mut self.tracks {
            let plane = track
                .plane
                .get_mut(..left.len())
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
            track
                .synth
                .process(&mut [plane], filtered, self.sample_rate)?;
            hosts.process(track.key, plane, filtered, self.sample_rate);
            add_stored_clips(&track.clips, spans, plane)?;
            crate::mix::add_prepared_mono(plane, track.fader, left, right);
        }
        Ok(())
    }
}

fn add_stored_clips(
    clips: &[StoredClip],
    spans: &[Span],
    output: &mut [f32],
) -> Result<(), PlaybackError> {
    for clip in clips {
        let view = [crate::voice::FadedClip {
            start_sample: clip.start_sample,
            samples: clip.samples.as_ref(),
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
        }];
        for span in spans {
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
            let plugins = track
                .devices
                .iter()
                .filter_map(|device| device.plugin.clone())
                .collect();
            Some(TrackChain {
                key: planned.track_key,
                plugins,
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

pub struct PlaybackControl {
    plans: Producer<Box<Prepared>>,
    returns: Consumer<Box<Prepared>>,
    commands: Producer<PlaybackCommand>,
    held: Option<Box<Prepared>>,
    host_plans: Producer<Box<HostGraph>>,
    host_returns: Consumer<Box<HostGraph>>,
    pending_hosts: Option<Box<HostGraph>>,
    chain: Vec<TrackChain>,
    routes: Vec<PluginRoute>,
    host_bin: std::path::PathBuf,
    ids: PlaybackIds,
    rate: u32,
    frames: usize,
    status: Arc<Status>,
    clip_loader: ClipLoader,
}

/// Owned by exactly one callback. Move back to the control thread before Drop.
pub struct PlaybackAudio {
    plans: Consumer<Box<Prepared>>,
    returns: Producer<Box<Prepared>>,
    commands: Consumer<PlaybackCommand>,
    host_plans: Consumer<Box<HostGraph>>,
    host_returns: Producer<Box<HostGraph>>,
    hosts: HostGraph,
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
    let (mut control, mut audio) =
        playback_with_clock(project, rate, frames, Box::<InternalClock>::default())?;
    let chain = chain_of(project, &audio.current);
    audio.hosts = HostGraph::open(
        host_bin,
        routes,
        &chain,
        frames,
        audio.current.events.len(),
        rate,
    )?;
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
    let mut current = Box::new(Prepared::new(project, rate, frames, &mut ids, &[])?);
    current.place(clock.position(), false);
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
    let control = PlaybackControl {
        plans,
        returns,
        commands,
        held: None,
        host_plans,
        host_returns,
        pending_hosts: None,
        chain: Vec::new(),
        routes: Vec::new(),
        host_bin: std::path::PathBuf::new(),
        ids,
        rate,
        frames,
        status: status.clone(),
        clip_loader: Arc::new(empty_clips),
    };
    let audio = PlaybackAudio {
        plans: incoming,
        returns: returning,
        commands: actions,
        host_plans: host_incoming,
        host_returns: host_returning,
        hosts: HostGraph::empty(),
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
        let prepared = Prepared::new(project, self.rate, self.frames, &mut self.ids, &loaded)?;
        let chain = chain_of(project, &prepared);
        if chain != self.chain && !self.host_bin.as_os_str().is_empty() {
            let hosts = HostGraph::open(
                &self.host_bin,
                &self.routes,
                &chain,
                self.frames,
                prepared.events.len(),
                self.rate,
            )?;
            self.chain = chain;
            self.pending_hosts = Some(Box::new(hosts));
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
        if self.returns.slots() > 0
            && let Ok(mut next) = self.plans.pop()
        {
            next.carry(&mut self.current, self.clock.position());
            std::mem::swap(&mut next, &mut self.current);
            // Only this thread writes the ring; the reserved slot cannot disappear.
            if let Err(rtrb::PushError::Full(_)) = self.returns.push(next) {
                std::process::abort();
            }
        }
        if self.host_returns.slots() > 0
            && let Ok(next) = self.host_plans.pop()
        {
            let previous = std::mem::replace(&mut self.hosts, *next);
            if let Err(rtrb::PushError::Full(_)) = self.host_returns.push(Box::new(previous)) {
                std::process::abort();
            }
        }
        while let Ok(command) = self.commands.pop() {
            match command {
                PlaybackCommand::Play => {
                    self.playing = true;
                    self.failed = false;
                    self.current.place(self.clock.position(), false);
                }
                PlaybackCommand::Pause | PlaybackCommand::Stop => {
                    // Both leave the playhead in place. The next play resumes there.
                    self.playing = false;
                    self.current.place(self.clock.position(), true);
                }
                PlaybackCommand::Locate(sample) => {
                    self.clock.locate(sample);
                    self.current.place(sample, true);
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
        self.current.render(spans, left, right, &mut self.hosts)?;
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
}
