//! One helper process per CLAP device, prepared on the control thread.
//!
//! The callback only submits and collects. A missed deadline silences that
//! track for the block. Stereo plugin output is averaged back to the track's
//! mono plane, then the existing fader pans it.

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
        for chain in chains {
            let mut stages = Vec::new();
            for plugin in &chain.plugins {
                let Some(route) = routes.iter().find(|route| route.clap_id == *plugin) else {
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
                    fault: None,
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
                });
            }
        }
        Ok(Self { fan, tracks })
    }

    /// Replaces `mono` with the plugin chain. Returns whether a helper ran.
    ///
    /// The first instrument sees silence instead of the built-in synth. Later
    /// devices hear the mono mix of the previous device. One deadline covers
    /// every stage.
    pub(crate) fn process(
        &mut self,
        key: u32,
        mono: &mut [f32],
        events: &[Event],
        sample_rate: u32,
    ) -> bool {
        let Some(index) = self.tracks.iter().position(|track| track.key == key) else {
            return false;
        };
        if self.tracks[index].stages.is_empty() || sample_rate == 0 {
            return false;
        }
        let frames = mono.len();
        let nanos = u64::try_from(frames)
            .unwrap_or(u64::MAX)
            .saturating_mul(1_000_000_000)
            / u64::from(sample_rate);
        let deadline = Instant::now() + Duration::from_nanos(nanos);
        let fan = self.fan.clone();
        let mut heard_plugin = false;
        for stage_index in 0..self.tracks[index].stages.len() {
            let stage = &mut self.tracks[index].stages[stage_index];
            let frames = frames.min(stage.input[0].len());
            if stage.instrument && !heard_plugin {
                stage.input[0][..frames].fill(0.0);
                stage.input[1][..frames].fill(0.0);
            } else {
                stage.input[0][..frames].copy_from_slice(&mono[..frames]);
                stage.input[1][..frames].copy_from_slice(&mono[..frames]);
            }
            stage.routed.clear();
            for event in events {
                if stage.routed.len() == stage.routed.capacity() {
                    break;
                }
                stage.routed.push((*event, 0));
            }
            let generation = {
                let input = [&stage.input[0][..frames], &stage.input[1][..frames]];
                stage.host.submit(frames, &input, &stage.routed)
            };
            let Ok(generation) = generation else {
                mono[..frames].fill(0.0);
                return true;
            };
            let ready = wait_stage(
                fan.as_ref(),
                &mut self.tracks[index].stages[stage_index],
                generation,
                deadline,
                frames,
            );
            if !ready {
                mono[..frames].fill(0.0);
                return true;
            }
            let stage = &self.tracks[index].stages[stage_index];
            for (sample, index) in mono.iter_mut().take(frames).zip(0..) {
                *sample = 0.5 * (stage.output[0][index] + stage.output[1][index]);
            }
            heard_plugin = true;
        }
        heard_plugin
    }
}

fn wait_stage(
    fan: Option<&Arc<FanIn>>,
    stage: &mut Stage,
    generation: u64,
    deadline: Instant,
    frames: usize,
) -> bool {
    let mut seen = fan.map(|fan| fan.load()).unwrap_or(0);
    loop {
        let Stage { host, output, .. } = stage;
        let (left, right) = output.split_at_mut(1);
        let progress = host.poll(
            generation,
            &mut [&mut left[0][..frames], &mut right[0][..frames]],
            &mut [],
        );
        match progress {
            Ok(BlockProgress::Ready { .. }) => return true,
            Ok(BlockProgress::Abandoned) => return false,
            Err(_) => {
                host.abandon(generation);
                return false;
            }
            Ok(BlockProgress::Pending) => {
                let Some(fan) = fan else {
                    host.abandon(generation);
                    return false;
                };
                if Instant::now() >= deadline {
                    host.abandon(generation);
                    return false;
                }
                let _ = fan.wait_until(seen, deadline);
                seen = fan.load();
            }
        }
    }
}
