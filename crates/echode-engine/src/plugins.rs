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

use echode_realtime::Event;
use echode_sandbox::{BlockProgress, FanIn, PluginHost, SessionLayout, SpawnOptions};

use crate::PlaybackError;

/// Where a CLAP id lives on disk, and whether it replaces the built-in synth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRoute {
    pub clap_id: String,
    pub path: PathBuf,
    pub instrument: bool,
    /// Passed to that helper as `ECHODE_TEST_FAULT`. Product routes leave this empty.
    pub fault: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackChain {
    pub key: u32,
    pub plugins: Vec<String>,
}

pub(crate) struct HostGraph {
    fan: Option<Arc<FanIn>>,
    tracks: Vec<TrackHost>,
}

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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Idle,
    Pending,
    Heard,
    Missed,
}

struct Stage {
    host: PluginHost,
    instrument: bool,
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
    routed: Vec<(Event, u32)>,
}

impl HostGraph {
    pub(crate) fn empty() -> Self {
        Self {
            fan: None,
            tracks: Vec::new(),
        }
    }

    /// Spawns a helper for every resolved device. An unknown id is skipped.
    /// A helper that fails to start is skipped too, so one missing plugin does
    /// not stop the other tracks.
    pub(crate) fn open(
        host_bin: &Path,
        routes: &[PluginRoute],
        chains: &[TrackChain],
        frames: usize,
        event_cap: usize,
        sample_rate: u32,
    ) -> Result<Self, PlaybackError> {
        if frames == 0 || frames > u32::MAX as usize {
            return Err(PlaybackError::InvalidSampleRate);
        }
        let mut tracks = Vec::new();
        let mut fan = None;
        let mut used = vec![false; routes.len()];
        for chain in chains {
            let mut stages = Vec::new();
            for plugin in &chain.plugins {
                let Some(route) = take_route(routes, &mut used, plugin) else {
                    continue;
                };
                if !route.path.is_file() {
                    continue;
                }
                let shared = if let Some(fan) = &fan {
                    Arc::clone(fan)
                } else {
                    let created = Arc::new(FanIn::create().map_err(|_| PlaybackError::Plugin)?);
                    fan = Some(Arc::clone(&created));
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
                    Ok(host) => host,
                    Err(_) => continue,
                };
                stages.push(Stage {
                    host,
                    instrument: route.instrument,
                    input: [vec![0.0; frames], vec![0.0; frames]],
                    output: [vec![0.0; frames], vec![0.0; frames]],
                    routed: Vec::with_capacity(event_cap.max(1)),
                });
            }
            if !stages.is_empty() {
                tracks.push(TrackHost {
                    key: chain.key,
                    stages,
                    stage: 0,
                    generation: 0,
                    frames: 0,
                    run: Run::Idle,
                    heard: false,
                    inject: vec![0.0; frames],
                    inject_live: false,
                });
            }
        }
        Ok(Self { fan, tracks })
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
        self.arm_inject(index, clips, frames);
        self.prepare_input(index, 0, Some(mono), events, frames);
        if self.submit_stage(index, 0, frames) {
            true
        } else {
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
                    Poll::Failed => self.tracks[index].run = Run::Missed,
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

    /// Smallest event buffer among the live helpers. An empty graph reports
    /// no limit, because there is nothing to grow.
    pub(crate) fn event_capacity(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| &track.stages)
            .map(|stage| {
                stage
                    .routed
                    .capacity()
                    .min(stage.host.layout().input_events as usize)
            })
            .min()
            .unwrap_or(usize::MAX)
    }

    /// Grows helper mappings and the routed lists. The plugin process stays up,
    /// so its audio state survives. Called on the control thread, between callbacks.
    pub(crate) fn ensure_events(&mut self, cap: usize) -> Result<(), PlaybackError> {
        let cap = cap.max(1);
        for track in &mut self.tracks {
            for stage in &mut track.stages {
                stage
                    .routed
                    .reserve(cap.saturating_sub(stage.routed.capacity()));
                let layout = stage.host.layout();
                if (layout.input_events as usize) >= cap {
                    continue;
                }
                let mut grown = layout;
                grown.input_events = u32::try_from(cap).unwrap_or(u32::MAX);
                stage.host.remap(grown).map_err(|_| PlaybackError::Plugin)?;
            }
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
                for (index, sample) in mono.iter_mut().take(frames).enumerate() {
                    let left = stage.output[0].get(index).copied().unwrap_or(0.0);
                    let right = stage.output[1].get(index).copied().unwrap_or(0.0);
                    *sample = 0.5 * (left + right);
                }
            }
            Run::Missed => mono[..frames].fill(0.0),
            Run::Idle | Run::Pending => {}
        }
    }

    fn prepare_input(
        &mut self,
        track: usize,
        stage_index: usize,
        source: Option<&[f32]>,
        events: &[Event],
        frames: usize,
    ) {
        let heard = self.tracks[track].heard;
        let stage = &mut self.tracks[track].stages[stage_index];
        let frames = frames.min(stage.input[0].len());
        // Notes still go out through `events`. Placed audio is not an input;
        // `arm_inject` puts it on this stage's output after the instrument runs.
        if stage.instrument && !heard {
            stage.input[0][..frames].fill(0.0);
            stage.input[1][..frames].fill(0.0);
        } else if let Some(source) = source {
            let frames = frames.min(source.len());
            stage.input[0][..frames].copy_from_slice(&source[..frames]);
            stage.input[1][..frames].copy_from_slice(&source[..frames]);
        }
        stage.routed.clear();
        for event in events {
            if stage.routed.len() == stage.routed.capacity() {
                break;
            }
            stage.routed.push((*event, 0));
        }
    }

    fn submit_stage(&mut self, track: usize, stage_index: usize, frames: usize) -> bool {
        let frames = frames.min(self.tracks[track].stages[stage_index].input[0].len());
        let generation = {
            let stage = &mut self.tracks[track].stages[stage_index];
            let input = [&stage.input[0][..frames], &stage.input[1][..frames]];
            stage.host.submit(frames, &input, &stage.routed)
        };
        match generation {
            Ok(generation) => {
                self.tracks[track].generation = generation;
                self.tracks[track].frames = frames;
                self.tracks[track].stage = stage_index;
                self.tracks[track].run = Run::Pending;
                true
            }
            Err(_) => false,
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
        let next = stage_index + 1;
        self.tracks[index].heard = true;
        if next >= self.tracks[index].stages.len() {
            self.tracks[index].run = Run::Heard;
            return;
        }
        if self.hand_off(index, stage_index, next, frames) {
            return;
        }
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
        let frames = frames
            .min(inject.len())
            .min(stage.output[0].len())
            .min(stage.output[1].len());
        let [left, right] = &mut stage.output;
        for ((left, right), add) in left[..frames]
            .iter_mut()
            .zip(right[..frames].iter_mut())
            .zip(inject[..frames].iter())
        {
            *left += *add;
            *right += *add;
        }
        *inject_live = false;
    }

    fn hand_off(&mut self, track: usize, from: usize, to: usize, frames: usize) -> bool {
        let stages = &mut self.tracks[track].stages;
        copy_mix(stages, from, to, frames);
        copy_routed(stages, from, to);
        self.submit_stage(track, to, frames)
    }

    fn abandon_pending(&mut self) {
        for track in &mut self.tracks {
            if track.run != Run::Pending {
                continue;
            }
            if let Some(stage) = track.stages.get_mut(track.stage) {
                stage.host.abandon(track.generation);
            }
            track.run = Run::Missed;
        }
    }
}

/// One absolute deadline for every helper in the callback.
pub(crate) fn block_deadline(frames: usize, sample_rate: u32) -> Instant {
    if sample_rate == 0 {
        return Instant::now();
    }
    let nanos = u64::try_from(frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(1_000_000_000)
        / u64::from(sample_rate);
    Instant::now() + Duration::from_nanos(nanos)
}

/// A later route with the same id is a separate helper. The last matching
/// route is reused when a project has more devices than routes.
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
    Failed,
    Pending,
}

fn poll_stage(stage: &mut Stage, generation: u64, frames: usize) -> Poll {
    let Stage { host, output, .. } = stage;
    let frames = frames.min(output[0].len()).min(output[1].len());
    let (left, right) = output.split_at_mut(1);
    match host.poll(
        generation,
        &mut [&mut left[0][..frames], &mut right[0][..frames]],
        &mut [],
    ) {
        Ok(BlockProgress::Ready { .. }) => Poll::Ready,
        Ok(BlockProgress::Pending) => Poll::Pending,
        Ok(BlockProgress::Abandoned) => Poll::Failed,
        Err(_) => {
            host.abandon(generation);
            Poll::Failed
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
    let frames = frames
        .min(source.output[0].len())
        .min(source.output[1].len())
        .min(dest.input[0].len())
        .min(dest.input[1].len());
    for index in 0..frames {
        let mixed = 0.5 * (source.output[0][index] + source.output[1][index]);
        dest.input[0][index] = mixed;
        dest.input[1][index] = mixed;
    }
}

fn copy_routed(stages: &mut [Stage], from: usize, to: usize) {
    let (head, tail) = stages.split_at_mut(to);
    let Some(source) = head.get(from) else {
        return;
    };
    let Some(dest) = tail.first_mut() else {
        return;
    };
    dest.routed.clear();
    for event in &source.routed {
        if dest.routed.len() == dest.routed.capacity() {
            break;
        }
        dest.routed.push(*event);
    }
}
