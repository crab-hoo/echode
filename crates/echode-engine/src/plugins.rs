//! One helper process per CLAP device, prepared on the control thread.
//!
//! The callback submits every independent track, then collects them until one
//! deadline. A missed deadline silences that track for the block. Stereo plugin
//! output is averaged back to the track's mono plane, then the existing fader
//! pans it.
//!
//! Effects hear the built-in synth plus placed audio. The first instrument
//! hears silence and the track's notes; placed audio is added to its output,
//! so a later effect hears the instrument and the clips together.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use echode_realtime::{Event, event_cmp, sort_events};
use echode_sandbox::{BlockProgress, FanIn, PluginHost, PluginNotice, SessionLayout, SpawnOptions};

use crate::PlaybackError;

/// Where a CLAP id lives on disk, and whether it replaces the built-in synth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRoute {
    pub clap_id: String,
    pub path: PathBuf,
    pub instrument: bool,
    /// Passed to that helper as `ECHODE_TEST_FAULT`. Product routes leave this empty.
    pub fault: Option<String>,
    /// CLAP state bytes loaded from the object store. `None` leaves the plugin's default.
    pub state: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChainDevice {
    pub id: u128,
    pub clap_id: String,
}

/// One track's CLAP devices, in project order. Identity is the device id, so an
/// unrelated track can be added without replacing a helper that is still sounding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackChain {
    pub key: u32,
    pub devices: Vec<ChainDevice>,
    /// Timeline events this track can emit in one block.
    pub events: usize,
    /// Chased note-ons for notes already sounding on this track.
    pub chase: usize,
}

pub(crate) struct HostGraph {
    fan: Option<Arc<FanIn>>,
    tracks: Vec<TrackHost>,
    /// A block tried to send more events than the routed buffer can hold.
    dropped_events: bool,
    /// Device whose plugin missed this block. Zero reason means none.
    /// Live playback still silences that block. Export reads the reason.
    export_device: u128,
    export_track: usize,
    export_index: usize,
    export_reason: u8,
}

const MISS_PROCESS: u8 = 1;
const MISS_DEADLINE: u8 = 2;
const MISS_SUBMIT: u8 = 3;
const MISS_EXITED: u8 = 4;

struct TrackHost {
    key: u32,
    stages: Vec<Stage>,
    /// Stage in flight, or the last stage copied back to the track.
    stage: usize,
    generation: u64,
    frames: usize,
    run: Run,
    /// A plugin on this track has finished during this callback.
    heard: bool,
    /// Placed audio to add onto the first instrument's output. Unused when the
    /// chain starts with an effect, because that effect already heard the clips.
    inject: Vec<f32>,
    inject_live: bool,
    /// This block's events, copied once so each stage can keep or skip note-ons.
    source_events: Vec<Event>,
    /// Note-ons the built-in synth already holds. Only a new helper receives them.
    chased_ons: Vec<Event>,
    chase_order: Vec<usize>,
    chase_scratch: Vec<Event>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Idle,
    Pending,
    Heard,
    Missed,
}

struct Stage {
    device: u128,
    clap_id: String,
    /// This process has not yet been told about notes the synth is already playing.
    awaiting_notes: bool,
    /// The buffer just built includes chased note-ons. Promoted when submit succeeds.
    chase_in_buffer: bool,
    /// The in-flight block included those note-ons. Cleared only after it is heard.
    chase_pending: bool,
    /// A chase was submitted and the block did not finish. Do not send it again.
    needs_restart: bool,
    /// This block's note-ons all fit, including a chase when one was still due.
    block_complete: bool,
    /// Bytes last restored into this process. `None` is the plugin's initial state.
    applied_state: Option<Vec<u8>>,
    /// A dirty notice arrived and its save has not been requested yet.
    dirty_again: bool,
    /// `SaveRequest` is in flight. The control thread does not wait for it.
    save_requested: bool,
    host: PluginHost,
    instrument: bool,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    /// Channels of the first output port. Those are the track's audio.
    main_outputs: usize,
    routed: Vec<(Event, u32)>,
}

impl HostGraph {
    pub(crate) fn empty() -> Self {
        Self {
            fan: None,
            tracks: Vec::new(),
            dropped_events: false,
            export_device: 0,
            export_track: 0,
            export_index: 0,
            export_reason: 0,
        }
    }

    pub(crate) fn dropped_events(&self) -> bool {
        self.dropped_events
    }

    /// Asks dirty helpers for their bytes without waiting. A reply is one save;
    /// a newer dirty notice stays unacked until the next reply.
    pub(crate) fn collect_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        let mut saved = Vec::new();
        for track in &mut self.tracks {
            for stage in &mut track.stages {
                if matches!(stage.host.poll_notice(), Ok(Some(PluginNotice::Dirty))) {
                    stage.dirty_again = true;
                }
                if stage.dirty_again && !stage.save_requested && stage.host.request_save().is_ok() {
                    stage.save_requested = true;
                    stage.dirty_again = false;
                }
                if !stage.save_requested {
                    continue;
                }
                match stage.host.poll_saved() {
                    Ok(Some(Ok(bytes))) => {
                        stage.save_requested = false;
                        stage.applied_state = Some(bytes.clone());
                        saved.push((stage.device, bytes));
                        if stage.dirty_again && stage.host.request_save().is_ok() {
                            stage.save_requested = true;
                            stage.dirty_again = false;
                        }
                    }
                    Ok(Some(Err(_))) | Err(_) => {
                        stage.save_requested = false;
                        stage.dirty_again = false;
                    }
                    Ok(None) => {}
                }
            }
        }
        saved
    }

    pub(crate) fn begin_block(&mut self) {
        self.export_reason = 0;
    }

    /// Why a plugin did not produce this block, including its device id.
    /// `None` when every submitted plugin finished. A live deadline miss is
    /// still silence for that block; this is what export refuses to call success.
    pub(crate) fn export_failure(&mut self) -> Option<String> {
        if self.export_reason == 0 {
            return None;
        }
        // A crash can outrun the exit watch inside one block. The callback
        // already recorded a deadline miss; the control thread can see the
        // process is gone. A hang is still running, so it stays a deadline miss.
        if self.export_reason == MISS_DEADLINE {
            let exited = self
                .tracks
                .get_mut(self.export_track)
                .and_then(|track| track.stages.get_mut(self.export_index))
                .is_some_and(|stage| stage.host.has_exited());
            if exited {
                self.export_reason = MISS_EXITED;
            }
        }
        let stage = self
            .tracks
            .get(self.export_track)
            .and_then(|track| track.stages.get(self.export_index));
        let Some(stage) = stage else {
            return Some("plugin failed".to_string());
        };
        let why = match self.export_reason {
            MISS_PROCESS => "process refused",
            MISS_DEADLINE => "did not finish before the deadline",
            MISS_SUBMIT => "could not submit audio",
            _ => "plugin process has exited",
        };
        Some(format!(
            "device {} ({}): {why}",
            stage.device, stage.clap_id
        ))
    }

    fn note_miss(&mut self, track: usize, stage_index: usize, reason: u8) {
        if self.export_reason != 0 {
            return;
        }
        let Some(stage) = self
            .tracks
            .get(track)
            .and_then(|track| track.stages.get(stage_index))
        else {
            self.export_reason = reason;
            return;
        };
        self.export_device = stage.device;
        self.export_track = track;
        self.export_index = stage_index;
        self.export_reason = reason;
    }

    /// Spawns a helper for every resolved device. An unknown id is skipped.
    /// A helper that fails to start is skipped too, so one missing plugin does
    /// not stop the other tracks.
    pub(crate) fn open(
        host_bin: &Path,
        routes: &[PluginRoute],
        chains: &[TrackChain],
        frames: usize,
        sample_rate: u32,
    ) -> Result<Self, PlaybackError> {
        let mut graph = Self::empty();
        graph.reconcile(host_bin, routes, chains, frames, sample_rate)?;
        Ok(graph)
    }

    /// Keeps a helper whose device id and CLAP id are unchanged. A new device is
    /// spawned and still needs the notes the built-in synth is already playing.
    /// Removed devices are dropped with their processes. An unchanged device is
    /// not spawned again.
    pub(crate) fn reconcile(
        &mut self,
        host_bin: &Path,
        routes: &[PluginRoute],
        chains: &[TrackChain],
        frames: usize,
        sample_rate: u32,
    ) -> Result<(), PlaybackError> {
        if frames == 0 || frames > u32::MAX as usize {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let mut kept = Vec::new();
        for track in self.tracks.drain(..) {
            for stage in track.stages {
                kept.push(stage);
            }
        }
        let mut tracks = Vec::new();
        let mut used = vec![false; routes.len()];
        for chain in chains {
            let mut stages = Vec::new();
            let delivery = chain
                .events
                .checked_add(chain.chase)
                .ok_or(PlaybackError::Time)?
                .max(1);
            for device in &chain.devices {
                if let Some(index) = kept
                    .iter()
                    .position(|stage| stage.device == device.id && stage.clap_id == device.clap_id)
                {
                    let incoming = unused_route(routes, &used, &device.clap_id)
                        .and_then(|route| route.state.clone());
                    if kept[index].applied_state != incoming {
                        // State changed. Leave the old process in `kept` so it is
                        // dropped, and spawn a helper that restores the new bytes.
                    } else {
                        let _ = take_route(routes, &mut used, &device.clap_id);
                        let stage = kept.swap_remove(index);
                        stages.push(stage);
                        continue;
                    }
                }
                match spawn_stage(
                    host_bin,
                    routes,
                    &mut used,
                    &mut self.fan,
                    device,
                    StageSize {
                        frames,
                        event_cap: delivery,
                        sample_rate,
                    },
                ) {
                    Ok(Some(stage)) => stages.push(stage),
                    Ok(None) => {}
                    Err(error) => {
                        self.tracks = tracks;
                        self.tracks.extend(rehome(kept));
                        return Err(error);
                    }
                }
            }
            for stage in &mut stages {
                fit_stage(stage, delivery)?;
            }
            if !stages.is_empty() {
                tracks.push(track_host(
                    chain.key,
                    stages,
                    frames,
                    chain.events.max(1),
                    chain.chase.max(1),
                ));
            }
        }
        self.tracks = tracks;
        Ok(())
    }

    /// Copies this track's input and publishes the first stage. Does not wait.
    ///
    /// `mono` is the built-in synth plus placed audio. `clips` is the placed
    /// audio alone. The first instrument ignores `mono` and receives notes;
    /// `clips` is added to that instrument's output. A track with no helper is
    /// left idle so the caller keeps `mono`.
    pub(crate) fn submit(
        &mut self,
        key: u32,
        mono: &[f32],
        clips: &[f32],
        events: &[Event],
        chased_ons: &[Event],
    ) -> bool {
        let Some(index) = self.tracks.iter().position(|track| track.key == key) else {
            return false;
        };
        self.tracks[index].heard = false;
        self.tracks[index].stage = 0;
        self.tracks[index].run = Run::Idle;
        if self.tracks[index].stages.is_empty() || mono.is_empty() {
            return false;
        }
        let frames = mono.len();
        self.remember(index, events, chased_ons);
        self.arm_inject(index, clips, frames);
        self.prepare_input(index, 0, Some(mono), frames);
        if self.submit_stage(index, 0, frames) {
            true
        } else {
            self.note_miss(index, 0, MISS_SUBMIT);
            self.tracks[index].run = Run::Missed;
            self.tracks[index].frames = frames;
            true
        }
    }

    /// Collects every track submitted for this callback.
    ///
    /// `deadline` is the one absolute time for the callback. A helper that is
    /// still running at that time is abandoned. A helper that already finished
    /// is kept, and its next stage is submitted and polled in this same collect.
    /// Leaving that stage pending would keep the dry track audio.
    pub(crate) fn collect(&mut self, deadline: Instant) {
        let mut seen = self.fan.as_ref().map(|fan| fan.load()).unwrap_or(0);
        loop {
            for index in 0..self.tracks.len() {
                if self.tracks[index].run != Run::Pending {
                    continue;
                }
                let stage_index = self.tracks[index].stage;
                let generation = self.tracks[index].generation;
                let frames = self.tracks[index].frames;
                match poll_stage(
                    &mut self.tracks[index].stages[stage_index],
                    generation,
                    frames,
                ) {
                    Poll::Ready => self.advance(index),
                    Poll::Failed(reason) => {
                        let stage_index = self.tracks[index].stage;
                        if let Some(stage) = self.tracks[index].stages.get_mut(stage_index)
                            && stage.chase_pending
                        {
                            stage.chase_pending = false;
                            stage.needs_restart = true;
                        }
                        self.note_miss(index, stage_index, reason);
                        self.tracks[index].run = Run::Missed;
                    }
                    Poll::Pending => {}
                }
            }
            // `advance` may have submitted the next stage. That stage is pending
            // even though this pass did not observe it waiting.
            if !self.tracks.iter().any(|track| track.run == Run::Pending) {
                return;
            }
            if Instant::now() >= deadline {
                self.abandon_pending();
                return;
            }
            let Some(fan) = &self.fan else {
                self.abandon_pending();
                return;
            };
            let _ = fan.wait_until(seen, deadline);
            seen = fan.load();
        }
    }

    /// Event slots reserved for each live track, in track order.
    pub(crate) fn track_event_caps(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .map(|track| track.source_events.capacity())
            .collect()
    }

    /// Drops helpers whose chased notes were submitted but not heard. The next
    /// reconcile starts a new process that still needs those notes.
    pub(crate) fn take_restarts(&mut self) -> bool {
        let mut restart = false;
        for track in &mut self.tracks {
            let before = track.stages.len();
            track.stages.retain(|stage| !stage.needs_restart);
            restart |= track.stages.len() != before;
        }
        self.tracks.retain(|track| !track.stages.is_empty());
        restart
    }

    /// Sizes one track's event buffers to its own demand, including a shrink.
    pub(crate) fn fit_track(
        &mut self,
        key: u32,
        events: usize,
        chase: usize,
    ) -> Result<(), PlaybackError> {
        let Some(track) = self.tracks.iter_mut().find(|track| track.key == key) else {
            return Ok(());
        };
        let events = events.max(1);
        let chase = chase.max(1);
        let delivery = events.checked_add(chase).ok_or(PlaybackError::Time)?;
        fit_vec(&mut track.source_events, events);
        fit_vec(&mut track.chased_ons, chase);
        fit_vec(&mut track.chase_order, chase);
        fit_vec(&mut track.chase_scratch, chase);
        for stage in &mut track.stages {
            fit_stage(stage, delivery)?;
        }
        Ok(())
    }

    /// Writes the collected plugin audio back. A miss silences the block.
    pub(crate) fn apply(&self, key: u32, mono: &mut [f32]) {
        let Some(track) = self.tracks.iter().find(|track| track.key == key) else {
            return;
        };
        let frames = mono.len().min(track.frames);
        match track.run {
            Run::Heard => {
                let Some(stage) = track.stages.get(track.stage) else {
                    mono[..frames].fill(0.0);
                    return;
                };
                let main = stage.main_outputs.min(stage.output.len()).max(1);
                for (index, sample) in mono.iter_mut().take(frames).enumerate() {
                    let mut mixed = 0.0f32;
                    for channel in stage.output.iter().take(main) {
                        mixed += channel.get(index).copied().unwrap_or(0.0);
                    }
                    *sample = mixed / main as f32;
                }
            }
            Run::Missed => mono[..frames].fill(0.0),
            Run::Idle | Run::Pending => {}
        }
    }

    fn remember(&mut self, track: usize, events: &[Event], chased_ons: &[Event]) {
        let key = self.tracks[track].key;
        let host = &mut self.tracks[track];
        let mut dropped = copy_events(&mut host.source_events, events, |_| true);
        host.chased_ons.clear();
        dropped |= copy_events(&mut host.chased_ons, chased_ons, |event| {
            event.track_key() == Some(key)
        });
        let chase_len = host.chased_ons.len();
        if chase_len > 0
            && !sort_events(
                &mut host.chased_ons[..chase_len],
                &mut host.chase_order,
                &mut host.chase_scratch,
            )
        {
            dropped = true;
        }
        if dropped {
            self.dropped_events = true;
        }
    }

    fn prepare_input(
        &mut self,
        track: usize,
        stage_index: usize,
        source: Option<&[f32]>,
        frames: usize,
    ) {
        let heard = self.tracks[track].heard;
        let stage = &mut self.tracks[track].stages[stage_index];
        let frames = bounded(&stage.input, frames);
        // Notes still go out through `events`. Placed audio is not an input;
        // `arm_inject` puts it on this stage's output after the instrument runs.
        if stage.instrument && !heard {
            for plane in &mut stage.input {
                let frames = frames.min(plane.len());
                plane[..frames].fill(0.0);
            }
        } else if let Some(source) = source {
            let frames = frames.min(source.len());
            for plane in &mut stage.input {
                let frames = frames.min(plane.len());
                plane[..frames].copy_from_slice(&source[..frames]);
            }
        }
        self.route_events(track, stage_index);
    }

    /// A helper that already received a sounding note does not get that note-on
    /// again. A helper spawned while the note is held still does.
    fn route_events(&mut self, track: usize, stage_index: usize) {
        let dropped = {
            let host = &mut self.tracks[track];
            let TrackHost {
                source_events,
                chased_ons,
                stages,
                ..
            } = host;
            let Some(stage) = stages.get_mut(stage_index) else {
                return;
            };
            let include_chase =
                stage.awaiting_notes && !stage.needs_restart && !stage.chase_pending;
            stage.routed.clear();
            let incoming = if include_chase { chased_ons.len() } else { 0 };
            let mut dropped = source_events.len() + incoming > stage.routed.capacity();
            let chase = if include_chase {
                chased_ons.as_slice()
            } else {
                &[]
            };
            let mut source_at = 0usize;
            let mut chase_at = 0usize;
            while source_at < source_events.len() || chase_at < chase.len() {
                if stage.routed.len() == stage.routed.capacity() {
                    dropped = true;
                    break;
                }
                let take_chase = match (source_events.get(source_at), chase.get(chase_at)) {
                    (None, Some(_)) => true,
                    (Some(_), None) => false,
                    (Some(source), Some(chased)) => {
                        event_cmp(chased, source) != std::cmp::Ordering::Greater
                    }
                    (None, None) => break,
                };
                let event = if take_chase {
                    chase_at += 1;
                    chase[chase_at - 1]
                } else {
                    source_at += 1;
                    source_events[source_at - 1]
                };
                stage.routed.push((event, 0));
            }
            stage.chase_in_buffer = include_chase && !dropped && !chase.is_empty();
            // A note-on that starts in this block is ordinary source, not chase.
            // Once that block is heard, the helper already has the voice, so a
            // later chase must not play it again.
            stage.block_complete = !dropped && (include_chase || !stage.awaiting_notes);
            dropped
        };
        if dropped {
            self.dropped_events = true;
        }
    }

    fn submit_stage(&mut self, track: usize, stage_index: usize, frames: usize) -> bool {
        let frames = {
            let stage = &self.tracks[track].stages[stage_index];
            bounded(&stage.input, frames.min(bounded(&stage.output, frames)))
        };
        let generation = {
            let stage = &mut self.tracks[track].stages[stage_index];
            stage
                .host
                .submit_planes(frames, &stage.input, &stage.routed)
        };
        let stage = &mut self.tracks[track].stages[stage_index];
        match generation {
            Ok(generation) => {
                if stage.chase_in_buffer {
                    stage.chase_pending = true;
                }
                stage.chase_in_buffer = false;
                self.tracks[track].generation = generation;
                self.tracks[track].frames = frames;
                self.tracks[track].stage = stage_index;
                self.tracks[track].run = Run::Pending;
                true
            }
            Err(_) => {
                stage.chase_in_buffer = false;
                false
            }
        }
    }

    fn arm_inject(&mut self, index: usize, clips: &[f32], frames: usize) {
        let track = &mut self.tracks[index];
        let instrument = track.stages.first().is_some_and(|stage| stage.instrument);
        if !instrument {
            track.inject_live = false;
            return;
        }
        let frames = frames.min(track.inject.len());
        for (slot, clip) in track.inject.iter_mut().take(frames).zip(clips) {
            *slot = *clip;
        }
        let copied = frames.min(clips.len());
        track.inject[copied..frames].fill(0.0);
        track.inject_live = true;
    }

    fn advance(&mut self, index: usize) {
        let frames = self.tracks[index].frames;
        let stage_index = self.tracks[index].stage;
        self.mix_injected(index, stage_index, frames);
        if let Some(stage) = self.tracks[index].stages.get_mut(stage_index) {
            if stage.block_complete {
                stage.awaiting_notes = false;
            }
            stage.chase_pending = false;
            stage.block_complete = false;
        }
        let next = stage_index + 1;
        self.tracks[index].heard = true;
        if next >= self.tracks[index].stages.len() {
            self.tracks[index].run = Run::Heard;
            return;
        }
        if self.hand_off(index, stage_index, next, frames) {
            return;
        }
        self.note_miss(index, next, MISS_SUBMIT);
        self.tracks[index].run = Run::Missed;
    }

    /// Adds placed audio onto the first instrument, once, before the next effect
    /// or the track output reads that buffer.
    fn mix_injected(&mut self, index: usize, stage_index: usize, frames: usize) {
        let track = &mut self.tracks[index];
        if !track.inject_live {
            return;
        }
        let TrackHost {
            stages,
            inject,
            inject_live,
            ..
        } = track;
        let Some(stage) = stages.get_mut(stage_index) else {
            return;
        };
        if !stage.instrument {
            return;
        }
        let main = stage.main_outputs.min(stage.output.len());
        let frames = bounded(&stage.output[..main], frames.min(inject.len()));
        for plane in stage.output.iter_mut().take(main) {
            let frames = frames.min(plane.len());
            for (sample, add) in plane[..frames].iter_mut().zip(inject[..frames].iter()) {
                *sample += *add;
            }
        }
        *inject_live = false;
    }

    fn hand_off(&mut self, track: usize, from: usize, to: usize, frames: usize) -> bool {
        {
            let stages = &mut self.tracks[track].stages;
            copy_mix(stages, from, to, frames);
        }
        self.route_events(track, to);
        self.submit_stage(track, to, frames)
    }

    fn abandon_pending(&mut self) {
        for index in 0..self.tracks.len() {
            if self.tracks[index].run != Run::Pending {
                continue;
            }
            let stage_index = self.tracks[index].stage;
            let generation = self.tracks[index].generation;
            let frames = self.tracks[index].frames;
            // A plugin that already refused or exited is not a deadline miss,
            // even when this callback has used up its block time.
            let discovered = {
                let stage = &mut self.tracks[index].stages[stage_index];
                poll_stage(stage, generation, frames)
            };
            if let Poll::Failed(reason) = discovered {
                self.note_miss(index, stage_index, reason);
                self.tracks[index].run = Run::Missed;
                continue;
            }
            if let Some(stage) = self.tracks[index].stages.get_mut(stage_index) {
                stage.host.abandon(generation);
            }
            self.note_miss(index, stage_index, MISS_DEADLINE);
            self.tracks[index].run = Run::Missed;
        }
    }
}

/// How long one block of `frames` lasts at `sample_rate`.
pub(crate) fn block_duration(frames: usize, sample_rate: u32) -> Duration {
    if sample_rate == 0 || frames == 0 {
        return Duration::ZERO;
    }
    let nanos = u64::try_from(frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000_000_000)
        / u64::from(sample_rate);
    Duration::from_nanos(nanos)
}

/// Absolute end of a callback that starts now. The origin is the callback
/// entry, so work already done is not added on again.
pub(crate) fn callback_deadline(frames: usize, sample_rate: u32) -> Instant {
    Instant::now() + block_duration(frames, sample_rate)
}

/// When helpers must be finished so the fader, mix, and metronome still fit
/// before `callback`. One eighth of this block is that tail. It is a share of
/// the block, not a limit on how many tracks the mix may contain.
pub(crate) fn collect_deadline(callback: Instant, frames: usize, sample_rate: u32) -> Instant {
    let reserve = block_duration(frames, sample_rate) / 8;
    callback.checked_sub(reserve).unwrap_or(callback)
}

fn track_host(
    key: u32,
    stages: Vec<Stage>,
    frames: usize,
    events: usize,
    chase: usize,
) -> TrackHost {
    TrackHost {
        key,
        stages,
        stage: 0,
        generation: 0,
        frames: 0,
        run: Run::Idle,
        heard: false,
        inject: vec![0.0; frames],
        inject_live: false,
        source_events: Vec::with_capacity(events.max(1)),
        chased_ons: Vec::with_capacity(chase.max(1)),
        chase_order: vec![0; chase.max(1)],
        chase_scratch: vec![Event::note_off(0, 0, 0, 0); chase.max(1)],
    }
}

fn rehome(stages: Vec<Stage>) -> Vec<TrackHost> {
    if stages.is_empty() {
        return Vec::new();
    }
    let frames = stages
        .first()
        .and_then(|stage| stage.input.first())
        .map(Vec::len)
        .unwrap_or(1);
    vec![track_host(0, stages, frames.max(1), 1, 1)]
}

fn fit_vec<T>(buf: &mut Vec<T>, cap: usize) {
    if buf.capacity() < cap {
        reserve_to(buf, cap);
    } else if buf.capacity() > cap {
        buf.shrink_to(cap.max(buf.len()));
    }
}

fn fit_stage(stage: &mut Stage, delivery: usize) -> Result<(), PlaybackError> {
    let delivery = u32::try_from(delivery).map_err(|_| PlaybackError::BufferTooSmall)?;
    fit_vec(&mut stage.routed, delivery as usize);
    let layout = stage.host.layout();
    if layout.input_events == delivery {
        return Ok(());
    }
    let mut next = layout;
    next.input_events = delivery;
    stage.host.remap(next).map_err(|_| PlaybackError::Plugin)?;
    Ok(())
}

/// Spawns one device. `Ok(None)` is an unknown id or a path that is not a file.
/// A file that cannot be hosted is an error, so playback does not pretend the
/// built-in synth is that plugin.
struct StageSize {
    frames: usize,
    event_cap: usize,
    sample_rate: u32,
}

fn spawn_stage(
    host_bin: &Path,
    routes: &[PluginRoute],
    used: &mut [bool],
    fan: &mut Option<Arc<FanIn>>,
    device: &ChainDevice,
    size: StageSize,
) -> Result<Option<Stage>, PlaybackError> {
    let StageSize {
        frames,
        event_cap,
        sample_rate,
    } = size;
    let Some(route) = take_route(routes, used, &device.clap_id) else {
        return Ok(None);
    };
    if !route.path.is_file() {
        return Ok(None);
    }
    let shared = if let Some(fan) = fan {
        Arc::clone(fan)
    } else {
        let created = Arc::new(FanIn::create().map_err(|_| PlaybackError::Plugin)?);
        *fan = Some(Arc::clone(&created));
        created
    };
    let layout = SessionLayout {
        max_frames: frames as u32,
        input_channels: 2,
        output_channels: 2,
        input_events: u32::try_from(event_cap.max(1)).unwrap_or(u32::MAX),
        output_events: 1,
    };
    let host = match PluginHost::spawn(SpawnOptions {
        host_bin,
        plugin: Some(route.path.as_path()),
        fault: route.fault.as_deref(),
        workgroup: None,
        layout,
        fan_in: Some(shared),
        sample_rate,
        plugin_id: Some(route.clap_id.as_str()),
    }) {
        Ok(mut host) => {
            if let Some(bytes) = &route.state {
                host.restore_state(bytes)
                    .map_err(|error| PlaybackError::Unsupported(error.to_string()))?;
            }
            host
        }
        Err(error) => {
            let text = error.to_string();
            if route.path.is_file() {
                return Err(PlaybackError::Unsupported(format!(
                    "device {} ({}): {text}",
                    device.id, device.clap_id
                )));
            }
            return Ok(None);
        }
    };
    let inputs = host.layout().input_channels as usize;
    let outputs = host.layout().output_channels as usize;
    let main_outputs = host
        .output_ports()
        .first()
        .copied()
        .unwrap_or(outputs as u32) as usize;
    Ok(Some(Stage {
        device: device.id,
        clap_id: device.clap_id.clone(),
        awaiting_notes: true,
        block_complete: false,
        chase_in_buffer: false,
        chase_pending: false,
        needs_restart: false,
        applied_state: route.state.clone(),
        dirty_again: false,
        save_requested: false,
        host,
        instrument: route.instrument,
        input: (0..inputs).map(|_| vec![0.0; frames]).collect(),
        output: (0..outputs).map(|_| vec![0.0; frames]).collect(),
        main_outputs: main_outputs.min(outputs),
        routed: Vec::with_capacity(event_cap.max(1)),
    }))
}

/// A later route with the same id is a separate helper. The last matching
/// route is reused when a project has more devices than routes.
fn unused_route<'a>(
    routes: &'a [PluginRoute],
    used: &[bool],
    plugin: &str,
) -> Option<&'a PluginRoute> {
    routes.iter().enumerate().find_map(|(index, route)| {
        (used.get(index).copied() == Some(false) && route.clap_id == plugin).then_some(route)
    })
}

fn take_route<'a>(
    routes: &'a [PluginRoute],
    used: &mut [bool],
    plugin: &str,
) -> Option<&'a PluginRoute> {
    if let Some(index) = routes.iter().enumerate().position(|(index, route)| {
        used.get(index).copied() == Some(false) && route.clap_id == plugin
    }) {
        if let Some(slot) = used.get_mut(index) {
            *slot = true;
        }
        return routes.get(index);
    }
    routes.iter().find(|route| route.clap_id == plugin)
}

enum Poll {
    Ready,
    Failed(u8),
    Pending,
}

fn bounded(planes: &[Vec<f32>], frames: usize) -> usize {
    match planes.iter().map(Vec::len).min() {
        Some(len) => frames.min(len),
        None => frames,
    }
}

fn poll_stage(stage: &mut Stage, generation: u64, frames: usize) -> Poll {
    let frames = bounded(&stage.output, frames);
    match stage
        .host
        .poll_planes(generation, &mut stage.output, frames, &mut [])
    {
        Ok(BlockProgress::Ready { .. }) => Poll::Ready,
        Ok(BlockProgress::Pending) => Poll::Pending,
        Ok(BlockProgress::Abandoned) => Poll::Failed(MISS_DEADLINE),
        Err(error) => {
            stage.host.abandon(generation);
            let reason = if matches!(error, echode_sandbox::SessionError::ProcessRefused) {
                MISS_PROCESS
            } else {
                MISS_EXITED
            };
            Poll::Failed(reason)
        }
    }
}

fn copy_mix(stages: &mut [Stage], from: usize, to: usize, frames: usize) {
    let (head, tail) = stages.split_at_mut(to);
    let Some(source) = head.get(from) else {
        return;
    };
    let Some(dest) = tail.first_mut() else {
        return;
    };
    let main = source.main_outputs.min(source.output.len()).max(1);
    let frames = frames
        .min(bounded(
            &source.output[..main.min(source.output.len())],
            frames,
        ))
        .min(bounded(&dest.input, frames));
    for index in 0..frames {
        let mut mixed = 0.0f32;
        for channel in source.output.iter().take(main) {
            mixed += channel.get(index).copied().unwrap_or(0.0);
        }
        let mixed = mixed / main as f32;
        for channel in &mut dest.input {
            if let Some(sample) = channel.get_mut(index) {
                *sample = mixed;
            }
        }
    }
}

fn copy_events(dest: &mut Vec<Event>, source: &[Event], keep: impl Fn(&Event) -> bool) -> bool {
    dest.clear();
    let mut dropped = false;
    for event in source {
        if !keep(event) {
            continue;
        }
        if dest.len() == dest.capacity() {
            dropped = true;
            break;
        }
        dest.push(*event);
    }
    dropped
}

fn reserve_to<T>(buf: &mut Vec<T>, cap: usize) {
    if buf.capacity() < cap {
        buf.reserve(cap.saturating_sub(buf.len()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reserve_to_grows_past_a_buffer_that_already_has_spare_capacity() {
        let mut values = Vec::<u8>::with_capacity(4);
        let cap = values.capacity() + 3;
        super::reserve_to(&mut values, cap);
        assert!(values.capacity() >= cap);
    }
}
