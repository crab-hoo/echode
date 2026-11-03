//! One helper process per CLAP device, prepared on the control thread.
//!
//! The callback submits every independent track, then collects them until one
//! deadline. A missed deadline silences that track for the block. Main mono and
//! stereo buses preserve their layout through the chain and into the fader.
//!
//! Effects hear the upstream devices and placed audio. The first instrument
//! hears silence and the track's notes; placed audio is added to its output,
//! so a later effect hears the instrument and the clips together.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use echode_realtime::{Event, event_cmp, sort_events};
use echode_sandbox::{
    BlockProgress, FanIn, PluginAudio, PluginControl, PluginHost, SessionLayout, SpawnOptions,
};

use crate::PlaybackError;

/// Control-thread factory for a fresh send right on each live helper launch.
/// The captured owner retains the selected device's workgroup.
pub type PluginWorkgroup =
    Arc<dyn Fn() -> Result<echode_sandbox::WorkgroupHandoff, String> + Send + Sync>;
use crate::mix::Signal;

/// Where a CLAP id lives on disk, and its instrument role.
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
    pub bypassed: bool,
}

/// One track's CLAP devices, in project order. Identity is the device id, so an
/// unrelated track can be added without replacing a helper that is still sounding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackChain {
    pub key: u32,
    pub stereo: bool,
    pub devices: Vec<ChainDevice>,
    /// Timeline events this track can emit in one block.
    pub events: usize,
    /// Chased note-ons for notes already sounding on this track.
    pub chase: usize,
}

pub(crate) struct HostGraph {
    fan: Option<Arc<FanIn>>,
    tracks: HostTracks,
    stage_index: Vec<(u128, usize, usize)>,
    /// A block tried to send more events than the routed buffer can hold.
    dropped_events: bool,
    /// Device whose plugin missed this block. Zero reason means none.
    /// Live playback still silences that block. Export reads the reason.
    export_reason: u8,
    cleanup_remaining: usize,
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
    inject_right: Vec<f32>,
    inject_stereo: bool,
    inject_live: bool,
    /// This block's events, copied once so each stage can keep or skip note-ons.
    source_events: Vec<Event>,
    /// Note-ons retained helpers already hold. Only a new helper receives them.
    chased_ons: Vec<Event>,
    chase_order: Vec<usize>,
    chase_scratch: Vec<Event>,
}

/// Live tracks in chain order, and an index by key rebuilt with every change
/// to the list. The callback looks each track up without scanning the others,
/// so a block's lookups grow with the tracks rather than their square
/// (XCA-13). Only the control thread replaces or shortens the list.
#[derive(Default)]
struct HostTracks {
    list: Vec<TrackHost>,
    /// `(key, position)`, sorted by key. Keys are unique within one chain.
    by_key: Vec<(u32, usize)>,
}

impl HostTracks {
    fn new(list: Vec<TrackHost>) -> Self {
        let mut by_key: Vec<_> = list
            .iter()
            .enumerate()
            .map(|(position, track)| (track.key, position))
            .collect();
        by_key.sort_unstable_by_key(|entry| entry.0);
        debug_assert!(
            by_key.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "two live tracks share a key"
        );
        Self { list, by_key }
    }

    fn position(&self, key: u32) -> Option<usize> {
        let index = self
            .by_key
            .binary_search_by_key(&key, |entry| entry.0)
            .ok()?;
        self.by_key.get(index).map(|entry| entry.1)
    }

    fn take(&mut self) -> Vec<TrackHost> {
        std::mem::take(self).list
    }

    fn retain(&mut self, keep: impl FnMut(&TrackHost) -> bool) {
        let mut list = self.take();
        list.retain(keep);
        *self = Self::new(list);
    }
}

impl std::ops::Deref for HostTracks {
    type Target = [TrackHost];

    fn deref(&self) -> &[TrackHost] {
        &self.list
    }
}

impl std::ops::DerefMut for HostTracks {
    fn deref_mut(&mut self) -> &mut [TrackHost] {
        &mut self.list
    }
}

struct TrackRestore {
    key: u32,
    devices: Vec<(u128, bool, usize)>,
    frames: usize,
    events: usize,
    chase: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Idle,
    Pending,
    Heard,
    Missed,
}

#[derive(Clone, Copy)]
enum Cleanup {
    None,
    Queued(Instant),
    Pending { generation: u64, started: Instant },
}

struct Stage {
    device: u128,
    clap_id: String,
    bypassed: bool,
    /// Dry main planes follow the upstream bus even if the wet port shape differs.
    bypass_output: Vec<Vec<f32>>,
    /// This process has not yet been told about notes already held on this track.
    awaiting_notes: bool,
    /// The buffer just built includes chased note-ons. Promoted when submit succeeds.
    chase_in_buffer: bool,
    /// The in-flight block included those note-ons. Cleared only after it is heard.
    chase_pending: bool,
    /// A chase was submitted and the block did not finish. Do not send it again.
    needs_restart: bool,
    /// This block's note-ons all fit, including a chase when one was still due.
    block_complete: bool,
    cleanup: Cleanup,
    host: PluginAudio,
    instance: Arc<Instance>,
    instrument: bool,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    /// Channels of the first output port. Those are the track's audio.
    main_outputs: usize,
    main_inputs: usize,
    routed: Vec<(Event, u32)>,
}

// Process lifetime and preparation belong to control. Audio holds a lease, but
// never locks it. Graph retirement returns the lease to control before Drop.
struct Instance {
    host: Mutex<PluginHost>,
    control: PluginControl,
    restart: AtomicBool,
    failure: AtomicU8,
}

impl Stage {
    fn rendered(&self) -> &[Vec<f32>] {
        if self.bypassed {
            &self.bypass_output
        } else {
            &self.output[..self.main_outputs.min(self.output.len())]
        }
    }
}

pub(crate) struct PreparationCheckpoint(Vec<(Arc<Instance>, bool)>);

pub(crate) struct ReconcileRoutes<'a> {
    pub entries: &'a [PluginRoute],
    pub restore_states: bool,
    /// Devices whose state matched at submission. Later live edits to those
    /// helpers must not turn an unrelated preparation into a stale reload.
    pub preserve: &'a [u128],
}

impl Instance {
    fn host(&self) -> std::sync::MutexGuard<'_, PluginHost> {
        self.host.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl HostGraph {
    pub(crate) fn empty() -> Self {
        Self {
            fan: None,
            tracks: HostTracks::default(),
            stage_index: Vec::new(),
            dropped_events: false,
            export_reason: 0,
            cleanup_remaining: 0,
        }
    }

    pub(crate) fn dropped_events(&self) -> bool {
        self.dropped_events
    }

    /// Snapshot control endpoints from the control-owned process catalog.
    /// Routine state polling uses these handles without taking the audio graph.
    pub(crate) fn save_controls(&self) -> Vec<(u128, PluginControl)> {
        self.tracks
            .iter()
            .flat_map(|track| &track.stages)
            .map(|stage| (stage.device, stage.instance.control.clone()))
            .collect()
    }

    pub(crate) fn missed_block(&self) -> bool {
        self.export_reason != 0
    }

    pub(crate) fn begin_block(&mut self) {
        for stage in self.tracks.iter().flat_map(|track| &track.stages) {
            stage.instance.failure.store(0, Ordering::Release);
        }
        self.export_reason = 0;
    }

    /// Mark every live processor, including downstream effects. Each helper
    /// resets on its own audio thread before the next block's events.
    pub(crate) fn reset_processing(&mut self) {
        for track in self.tracks.iter_mut() {
            for stage in &mut track.stages {
                stage.host.request_reset();
            }
        }
    }

    pub(crate) fn cleanup_pending(&self) -> bool {
        self.cleanup_remaining != 0
    }

    /// Discarded lifecycle processing has no audio dependency between stages.
    /// Reset every helper independently, retaining note-offs until publication.
    pub(crate) fn queue_cleanup(&mut self, events: &[Event]) -> bool {
        // No score bank or audible block can run until this batch completes.
        // A later pause/locate therefore asks to clear the same processing state.
        if self.cleanup_pending() {
            return true;
        }
        let started = Instant::now();
        let mut fits = true;
        for track in self.tracks.iter_mut() {
            track.source_events.clear();
        }
        // One pass over the events, so a stop with many held notes does not
        // scan them once per track (XCA-13).
        for event in events {
            let Some(position) = event.track_key().and_then(|key| self.tracks.position(key)) else {
                continue;
            };
            let held = &mut self.tracks[position].source_events;
            if held.len() == held.capacity() {
                fits = false;
                continue;
            }
            held.push(*event);
        }
        for track in self.tracks.iter_mut() {
            for stage in &mut track.stages {
                if stage.bypassed {
                    // No discarded process block is needed while dry. The
                    // pending reset is consumed when this helper is enabled.
                    stage.host.request_reset();
                    stage.awaiting_notes = true;
                    continue;
                }
                if !matches!(stage.cleanup, Cleanup::None) {
                    continue;
                }
                stage.routed.clear();
                for event in &track.source_events {
                    if stage.routed.len() == stage.routed.capacity() {
                        fits = false;
                        break;
                    }
                    stage.routed.push((*event, 0));
                }
                for plane in &mut stage.input {
                    if let Some(sample) = plane.first_mut() {
                        *sample = 0.0;
                    }
                }
                stage.host.request_reset();
                stage.cleanup = Cleanup::Queued(started);
                self.cleanup_remaining += 1;
            }
        }
        fits
    }

    /// Live callbacks pass no deadline and poll exactly once. A caller that
    /// switches this player back to offline mode can drain with a bounded wait.
    pub(crate) fn collect_cleanup(&mut self, deadline: Option<Instant>) -> bool {
        if !self.cleanup_pending() {
            return true;
        }
        self.begin_block();
        let mut seen = self.fan.as_ref().map(|fan| fan.load()).unwrap_or(0);
        loop {
            let now = Instant::now();
            for track in 0..self.tracks.len() {
                for index in 0..self.tracks[track].stages.len() {
                    let stage = &mut self.tracks[track].stages[index];
                    if matches!(stage.cleanup, Cleanup::None) {
                        continue;
                    }
                    if let Some(result) = poll_cleanup_stage(stage, now) {
                        self.cleanup_remaining -= 1;
                        if let Err(reason) = result {
                            self.note_miss(track, index, reason);
                        }
                    }
                }
            }
            if !self.cleanup_pending() {
                return true;
            }
            let Some(deadline) = deadline else {
                return false;
            };
            if Instant::now() >= deadline || self.fan.is_none() {
                for track in 0..self.tracks.len() {
                    for index in 0..self.tracks[track].stages.len() {
                        let stage = &mut self.tracks[track].stages[index];
                        if matches!(stage.cleanup, Cleanup::None) {
                            continue;
                        }
                        fail_cleanup(stage);
                        self.cleanup_remaining -= 1;
                        self.note_miss(track, index, MISS_DEADLINE);
                    }
                }
                return true;
            }
            if let Some(fan) = &self.fan {
                let _ = fan.wait_until(seen, deadline);
                seen = fan.load();
            }
        }
    }

    /// Why a plugin did not produce this block, including its device id.
    /// `None` when every submitted plugin finished. A live deadline miss is
    /// still silence for that block; this is what export refuses to call success.
    pub(crate) fn export_failure(&self) -> Option<String> {
        for stage in self.tracks.iter().flat_map(|track| &track.stages) {
            if stage.bypassed {
                continue;
            }
            let mut reason = stage.instance.failure.load(Ordering::Acquire);
            if reason == 0 {
                continue;
            }
            if reason == MISS_DEADLINE && stage.instance.host().has_exited() {
                reason = MISS_EXITED;
            }
            let why = match reason {
                MISS_PROCESS => "process refused",
                MISS_DEADLINE => "did not finish before the deadline",
                MISS_SUBMIT => "could not submit audio",
                _ => "plugin process has exited",
            };
            return Some(format!(
                "device {} ({}): {why}",
                stage.device, stage.clap_id
            ));
        }
        None
    }

    /// Allocate an audio-owned bank using the already prepared process endpoints.
    pub(crate) fn audio_snapshot(&self) -> Self {
        let mut graph = Self::empty();
        graph.fan = self.fan.clone();
        graph.tracks = HostTracks::new(
            self.tracks
                .iter()
                .map(|track| {
                    let stages = track
                        .stages
                        .iter()
                        .map(|stage| Stage {
                            device: stage.device,
                            clap_id: stage.clap_id.clone(),
                            bypassed: stage.bypassed,
                            bypass_output: stage.bypass_output.clone(),
                            awaiting_notes: true,
                            chase_in_buffer: false,
                            chase_pending: false,
                            needs_restart: false,
                            block_complete: false,
                            cleanup: Cleanup::None,
                            // SAFETY: only the current bank is processed; queued and
                            // retired banks are never submitted or collected.
                            host: unsafe { stage.instance.host().audio_handle() },
                            instance: Arc::clone(&stage.instance),
                            instrument: stage.instrument,
                            input: stage.input.clone(),
                            output: stage.output.clone(),
                            main_outputs: stage.main_outputs,
                            main_inputs: stage.main_inputs,
                            routed: Vec::with_capacity(stage.routed.capacity()),
                        })
                        .collect();
                    track_host(
                        track.key,
                        stages,
                        track.inject.len(),
                        track.source_events.capacity(),
                        track.chased_ons.capacity(),
                    )
                })
                .collect(),
        );
        graph.stage_index = graph
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(track, host)| {
                host.stages
                    .iter()
                    .enumerate()
                    .map(move |(stage, host)| (host.device, track, stage))
            })
            .collect();
        graph.stage_index.sort_unstable_by_key(|entry| entry.0);
        graph
    }

    /// Callback boundary only. Preserve DSP ownership and chase/reset receipts.
    pub(crate) fn carry(&mut self, old: &Self) {
        for stage in self.tracks.iter_mut().flat_map(|track| &mut track.stages) {
            let previous = old
                .stage_index
                .binary_search_by_key(&stage.device, |entry| entry.0)
                .ok()
                .and_then(|index| old.stage_index.get(index))
                .and_then(|(_, track, index)| old.tracks.get(*track)?.stages.get(*index));
            if let Some(previous) =
                previous.filter(|previous| Arc::ptr_eq(&previous.instance, &stage.instance))
            {
                stage.awaiting_notes = previous.awaiting_notes;
                stage.chase_in_buffer = previous.chase_in_buffer;
                stage.chase_pending = previous.chase_pending;
                stage.needs_restart = previous.needs_restart;
                stage.block_complete = previous.block_complete;
                stage.host.carry(&previous.host);
                if stage.bypassed != previous.bypassed {
                    // The helper may still hold voices from before bypass.
                    // Reset on its audio thread, then chase only current notes.
                    stage.host.request_reset();
                    stage.awaiting_notes = true;
                    stage.chase_in_buffer = false;
                    stage.chase_pending = false;
                    stage.block_complete = false;
                    stage.needs_restart = false;
                }
            }
        }
    }

    pub(crate) fn activate_prepared(&mut self) {
        for stage in self.tracks.iter_mut().flat_map(|track| &mut track.stages) {
            if stage.host.activate_mapping().is_err() {
                stage.instance.failure.store(MISS_SUBMIT, Ordering::Release);
            }
        }
    }

    /// A prepared bank must be adopted before another layout for that process
    /// can be prepared. Keep rendering the current bank while control retries.
    pub(crate) fn can_reconcile(&self, chains: &[TrackChain]) -> bool {
        chains.iter().all(|chain| {
            self.tracks
                .iter()
                .flat_map(|track| &track.stages)
                .all(|stage| {
                    if !chain.devices.iter().any(|device| device.id == stage.device) {
                        return true;
                    }
                    let host = stage.instance.host();
                    host.layout().input_events as usize
                        >= chain.events.saturating_add(chain.chase).max(1)
                        || !host.mapping_pending()
                })
        })
    }

    pub(crate) fn preparation_checkpoint(&self) -> PreparationCheckpoint {
        PreparationCheckpoint(
            self.tracks
                .iter()
                .flat_map(|track| &track.stages)
                .map(|stage| {
                    (
                        Arc::clone(&stage.instance),
                        stage.instance.host().mapping_pending(),
                    )
                })
                .collect(),
        )
    }

    pub(crate) fn cancel_preparation(
        &self,
        checkpoint: &PreparationCheckpoint,
    ) -> Result<(), PlaybackError> {
        for stage in self.tracks.iter().flat_map(|track| &track.stages) {
            let was_pending = checkpoint
                .0
                .iter()
                .any(|(instance, pending)| *pending && Arc::ptr_eq(instance, &stage.instance));
            if !was_pending {
                stage
                    .instance
                    .host()
                    .cancel_mapping()
                    .map_err(|_| PlaybackError::Plugin)?;
            }
        }
        Ok(())
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
        stage.instance.failure.store(reason, Ordering::Release);
        self.export_reason = reason;
    }

    pub(crate) fn tail_samples(&self) -> u32 {
        self.tracks
            .iter()
            .map(|track| {
                track
                    .stages
                    .iter()
                    .filter(|stage| !stage.bypassed)
                    .fold(0u32, |sum, stage| {
                        sum.saturating_add(stage.instance.host().tail_samples())
                    })
            })
            .max()
            .unwrap_or(0)
    }

    /// Devices whose recorded states still matched when a graph was submitted.
    pub(crate) fn matching_states(
        &self,
        chains: &[TrackChain],
        routes: &[PluginRoute],
    ) -> Vec<u128> {
        let mut used = vec![false; routes.len()];
        chains
            .iter()
            .flat_map(|chain| &chain.devices)
            .filter_map(|device| {
                let incoming = take_route(routes, &mut used, &device.clap_id)
                    .and_then(|route| route.state.as_ref());
                let stage = self
                    .tracks
                    .iter()
                    .flat_map(|track| &track.stages)
                    .find(|stage| stage.device == device.id && stage.clap_id == device.clap_id)?;
                (stage.instance.control.applied_state().as_ref() == incoming).then_some(device.id)
            })
            .collect()
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
        workgroup: Option<&PluginWorkgroup>,
    ) -> Result<Self, PlaybackError> {
        let mut graph = Self::empty();
        graph.reconcile(
            host_bin,
            ReconcileRoutes {
                entries: routes,
                restore_states: true,
                preserve: &[],
            },
            chains,
            frames,
            sample_rate,
            workgroup,
        )?;
        Ok(graph)
    }

    /// Keeps a helper whose device id and CLAP id are unchanged. A new device is
    /// spawned and still needs the notes the built-in synth is already playing.
    /// Removed devices are dropped with their processes. An unchanged device is
    /// not spawned again.
    pub(crate) fn reconcile(
        &mut self,
        host_bin: &Path,
        selection: ReconcileRoutes<'_>,
        chains: &[TrackChain],
        frames: usize,
        sample_rate: u32,
        workgroup: Option<&PluginWorkgroup>,
    ) -> Result<(), PlaybackError> {
        let routes = selection.entries;
        if frames == 0 || frames > u32::MAX as usize {
            return Err(PlaybackError::InvalidSampleRate);
        }
        // Keep the old ownership and ordering until every new mapping is valid.
        // A rejected layout must not disconnect helpers that are still sounding.
        let restore: Vec<_> = self
            .tracks
            .iter()
            .map(|track| TrackRestore {
                key: track.key,
                devices: track
                    .stages
                    .iter()
                    .map(|stage| (stage.device, stage.bypassed, stage.bypass_output.len()))
                    .collect(),
                frames: track.inject.len(),
                events: track.source_events.capacity(),
                chase: track.chased_ons.capacity(),
            })
            .collect();
        let deliveries = chains
            .iter()
            .map(|chain| {
                chain
                    .events
                    .checked_add(chain.chase)
                    .and_then(|n| n.max(1).checked_next_power_of_two())
                    .ok_or(PlaybackError::Time)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut kept = Vec::new();
        for track in self.tracks.take() {
            for stage in track.stages {
                kept.push(stage);
            }
        }
        let mut tracks = Vec::new();
        let mut used = vec![false; routes.len()];
        for (chain, delivery) in chains.iter().zip(deliveries) {
            let mut stages = Vec::new();
            for device in &chain.devices {
                if let Some(index) = kept
                    .iter()
                    .position(|stage| stage.device == device.id && stage.clap_id == device.clap_id)
                {
                    let incoming = unused_route(routes, &used, &device.clap_id)
                        .and_then(|route| route.state.clone());
                    if selection.restore_states
                        && !selection.preserve.contains(&device.id)
                        && kept[index].instance.control.applied_state() != incoming
                    {
                        // State changed. Leave the old process in `kept` so it is
                        // dropped, and spawn a helper that restores the new bytes.
                    } else {
                        let _ = take_route(routes, &mut used, &device.clap_id);
                        let mut stage = kept.swap_remove(index);
                        stage.bypassed = device.bypassed;
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
                        workgroup,
                    },
                ) {
                    Ok(Some(stage)) => stages.push(stage),
                    Ok(None) => {}
                    Err(error) => {
                        self.tracks =
                            HostTracks::new(restore_tracks(restore, tracks, stages, kept));
                        return Err(error);
                    }
                }
            }
            let result = validate_chain(&mut stages, chain.stereo).and_then(|()| {
                stages
                    .iter_mut()
                    .try_for_each(|stage| fit_stage(stage, delivery))
            });
            if let Err(error) = result {
                self.tracks = HostTracks::new(restore_tracks(restore, tracks, stages, kept));
                return Err(error);
            }
            if !stages.is_empty() {
                tracks.push(track_host(
                    chain.key,
                    stages,
                    frames,
                    delivery,
                    chain.chase.max(1),
                ));
            }
        }
        self.tracks = HostTracks::new(tracks);
        Ok(())
    }

    /// Copies this track's input and publishes the first stage. Does not wait.
    ///
    /// `source` contains placed audio. `clips` is the placed
    /// audio alone. The first instrument ignores `source` and receives notes;
    /// `clips` is added to that instrument's output. A track with no helper is
    /// left idle so the caller keeps `source`. `events` and `chased_ons` are
    /// this track's alone; the caller groups the block's events once.
    pub(crate) fn submit(
        &mut self,
        key: u32,
        source: Signal<'_>,
        clips: Signal<'_>,
        events: &[Event],
        chased_ons: &[Event],
    ) -> bool {
        let Some(index) = self.tracks.position(key) else {
            return false;
        };
        self.tracks[index].heard = false;
        self.tracks[index].stage = 0;
        self.tracks[index].run = Run::Idle;
        if self.tracks[index].stages.is_empty() || source.left.is_empty() {
            return false;
        }
        let frames = source.left.len();
        self.remember(index, events, chased_ons);
        self.arm_inject(index, clips, frames);
        self.prepare_input(index, 0, source, frames);
        if self.submit_stage(index, 0, frames) {
            true
        } else {
            self.note_miss(index, self.tracks[index].stage, MISS_SUBMIT);
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
                            stage.instance.restart.store(true, Ordering::Release);
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
            .map(|track| {
                track
                    .stages
                    .iter()
                    .map(|stage| stage.instance.host().layout().input_events as usize)
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    }

    /// Drops helpers whose chased notes were submitted but not heard. The next
    /// reconcile starts a new process that still needs those notes.
    pub(crate) fn take_restarts(&mut self) -> bool {
        let mut restart = false;
        for track in self.tracks.iter_mut() {
            let before = track.stages.len();
            track
                .stages
                .retain(|stage| stage.bypassed || !stage.instance.restart.load(Ordering::Acquire));
            restart |= track.stages.len() != before;
        }
        self.tracks.retain(|track| !track.stages.is_empty());
        restart
    }

    /// Writes the collected plugin audio back. A miss silences the block.
    pub(crate) fn apply(&self, key: u32, left: &mut [f32], right: &mut [f32]) -> Option<bool> {
        let track = self.tracks.get(self.tracks.position(key)?)?;
        let frames = left.len().min(right.len()).min(track.frames);
        let stage = track.stages.get(track.stage)?;
        let output = stage.rendered();
        let stereo = output.len() == 2;
        match track.run {
            Run::Heard => {
                left[..frames].copy_from_slice(&output[0][..frames]);
                if stereo {
                    right[..frames].copy_from_slice(&output[1][..frames]);
                }
            }
            Run::Missed => {
                left[..frames].fill(0.0);
                right[..frames].fill(0.0);
            }
            Run::Idle | Run::Pending => return None,
        }
        Some(stereo)
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
        source: Signal<'_>,
        frames: usize,
    ) {
        let heard = self.tracks[track].heard;
        let stage = &mut self.tracks[track].stages[stage_index];
        if stage.bypassed {
            if stage.instrument && !heard {
                crate::copy_bypassed_audio(std::iter::empty(), &mut stage.bypass_output, frames);
            } else {
                crate::copy_bypassed_audio(
                    std::iter::once(source.left).chain(source.right),
                    &mut stage.bypass_output,
                    frames,
                );
            }
            return;
        }
        let frames = bounded(&stage.input, frames);
        // Notes still go out through `events`. Placed audio is not an input;
        // `arm_inject` puts it on this stage's output after the instrument runs.
        if stage.instrument && !heard {
            for plane in &mut stage.input {
                let frames = frames.min(plane.len());
                plane[..frames].fill(0.0);
            }
        } else {
            for (channel, plane) in stage.input.iter_mut().enumerate() {
                for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                    *sample = if channel < stage.main_inputs {
                        source.sample(channel, frame, stage.main_inputs == 2)
                    } else {
                        0.0
                    };
                }
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
            if stage.bypassed {
                return;
            }
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

    fn submit_stage(&mut self, track: usize, mut stage_index: usize, mut frames: usize) -> bool {
        // A run of bypassed stages consumes no helper deadline and no stack
        // frames. The number of devices is not limited by recursive calls.
        loop {
            self.tracks[track].stage = stage_index;
            if !self.tracks[track].stages[stage_index].bypassed {
                break;
            }
            frames = bounded(
                &self.tracks[track].stages[stage_index].bypass_output,
                frames,
            );
            self.tracks[track].frames = frames;
            self.mix_injected(track, stage_index, frames);
            self.tracks[track].stages[stage_index].awaiting_notes = true;
            self.tracks[track].heard = true;
            let next = stage_index + 1;
            if next == self.tracks[track].stages.len() {
                self.tracks[track].run = Run::Heard;
                return true;
            }
            copy_mix(&mut self.tracks[track].stages, stage_index, next, frames);
            self.route_events(track, next);
            stage_index = next;
        }
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

    fn arm_inject(&mut self, index: usize, clips: Signal<'_>, frames: usize) {
        let track = &mut self.tracks[index];
        let instrument = track.stages.first().is_some_and(|stage| stage.instrument);
        if !instrument {
            track.inject_live = false;
            return;
        }
        let frames = frames.min(track.inject.len());
        for (frame, (left, right)) in track
            .inject
            .iter_mut()
            .zip(&mut track.inject_right)
            .take(frames)
            .enumerate()
        {
            *left = clips.left.get(frame).copied().unwrap_or(0.0);
            *right = clips
                .right
                .and_then(|right| right.get(frame))
                .copied()
                .unwrap_or(0.0);
        }
        track.inject_stereo = clips.right.is_some();
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
        self.note_miss(index, self.tracks[index].stage, MISS_SUBMIT);
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
            inject_right,
            inject_stereo,
            inject_live,
            ..
        } = track;
        let Some(stage) = stages.get_mut(stage_index) else {
            return;
        };
        if !stage.instrument {
            return;
        }
        let output = if stage.bypassed {
            &mut stage.bypass_output[..]
        } else {
            let main = stage.main_outputs.min(stage.output.len());
            &mut stage.output[..main]
        };
        let main = output.len();
        let frames = bounded(output, frames.min(inject.len()));
        let source = Signal {
            left: inject,
            right: inject_stereo.then_some(&*inject_right),
        };
        for (channel, plane) in output.iter_mut().enumerate() {
            let frames = frames.min(plane.len());
            for (frame, sample) in plane[..frames].iter_mut().enumerate() {
                *sample += source.sample(channel, frame, main == 2);
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
        inject_right: vec![0.0; frames],
        inject_stereo: false,
        inject_live: false,
        source_events: Vec::with_capacity(events.max(1)),
        chased_ons: Vec::with_capacity(chase.max(1)),
        chase_order: vec![0; chase.max(1)],
        chase_scratch: vec![Event::note_off(0, 0, 0, 0); chase.max(1)],
    }
}

fn restore_tracks(
    restore: Vec<TrackRestore>,
    tracks: Vec<TrackHost>,
    pending: Vec<Stage>,
    mut kept: Vec<Stage>,
) -> Vec<TrackHost> {
    kept.extend(pending);
    kept.extend(tracks.into_iter().flat_map(|track| track.stages));
    restore
        .into_iter()
        .map(|old| {
            let stages = old
                .devices
                .into_iter()
                .filter_map(|(id, bypassed, channels)| {
                    // A restored state may have spawned another helper with the same
                    // device id. `kept` starts with the original process in that case.
                    let at = kept.iter().position(|stage| stage.device == id)?;
                    let mut stage = kept.remove(at);
                    stage.bypassed = bypassed;
                    stage
                        .bypass_output
                        .resize_with(channels, || vec![0.0; old.frames]);
                    Some(stage)
                })
                .collect();
            track_host(old.key, stages, old.frames, old.events, old.chase)
        })
        .collect()
}

fn fit_stage(stage: &mut Stage, delivery: usize) -> Result<(), PlaybackError> {
    let delivery = u32::try_from(delivery).map_err(|_| PlaybackError::BufferTooSmall)?;
    reserve_to(&mut stage.routed, delivery as usize);
    let mut host = stage.instance.host();
    let layout = host.layout();
    if layout.input_events >= delivery {
        return Ok(());
    }
    let mut next = layout;
    next.input_events = delivery;
    host.prepare_mapping(next)
        .map_err(|error| PlaybackError::Unsupported(error.to_string()))?;
    Ok(())
}

/// Spawns one device. `Ok(None)` is an unknown id or a missing path.
/// An installed bundle/file that cannot be hosted is an error, so playback does not pretend the
/// a substitute instrument is that plugin.
struct StageSize<'a> {
    frames: usize,
    event_cap: usize,
    sample_rate: u32,
    workgroup: Option<&'a PluginWorkgroup>,
}

fn spawn_stage(
    host_bin: &Path,
    routes: &[PluginRoute],
    used: &mut [bool],
    fan: &mut Option<Arc<FanIn>>,
    device: &ChainDevice,
    size: StageSize<'_>,
) -> Result<Option<Stage>, PlaybackError> {
    let StageSize {
        frames,
        event_cap,
        sample_rate,
        workgroup,
    } = size;
    let Some(route) = take_route(routes, used, &device.clap_id) else {
        return Ok(None);
    };
    if !route.path.exists() {
        return Ok(None);
    }
    let shared = if let Some(fan) = fan {
        Arc::clone(fan)
    } else {
        let created = Arc::new(
            FanIn::create().map_err(|error| PlaybackError::Unsupported(error.to_string()))?,
        );
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
        workgroup: workgroup
            .map(|source| source())
            .transpose()
            .map_err(PlaybackError::Unsupported)?,
        layout,
        fan_in: Some(shared),
        sample_rate,
        plugin_id: Some(route.clap_id.as_str()),
    }) {
        Ok(host) => {
            if let Some(bytes) = &route.state {
                host.control_handle()
                    .restore_state(bytes)
                    .map_err(|error| PlaybackError::Unsupported(error.to_string()))?;
            }
            host
        }
        Err(error) => {
            let text = error.to_string();
            if route.path.exists() {
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
    let main_inputs = host.input_ports().first().copied().unwrap_or(inputs as u32) as usize;
    Ok(Some(Stage {
        device: device.id,
        clap_id: device.clap_id.clone(),
        bypassed: device.bypassed,
        bypass_output: Vec::new(),
        awaiting_notes: true,
        block_complete: false,
        cleanup: Cleanup::None,
        chase_in_buffer: false,
        chase_pending: false,
        needs_restart: false,
        // SAFETY: this catalog endpoint is never processed; audio snapshots
        // are used exclusively by the one playback callback.
        host: unsafe { host.audio_handle() },
        instance: Arc::new(Instance {
            control: host.control_handle(),
            host: Mutex::new(host),
            restart: AtomicBool::new(false),
            failure: AtomicU8::new(0),
        }),
        instrument: route.instrument,
        input: (0..inputs).map(|_| vec![0.0; frames]).collect(),
        output: (0..outputs).map(|_| vec![0.0; frames]).collect(),
        main_outputs: main_outputs.min(outputs),
        main_inputs: main_inputs.min(inputs),
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

fn fail_cleanup(stage: &mut Stage) {
    if let Cleanup::Pending { generation, .. } = stage.cleanup {
        stage.host.abandon(generation);
    }
    stage.cleanup = Cleanup::None;
    stage.instance.restart.store(true, Ordering::Release);
    stage.needs_restart = true;
}

fn poll_cleanup_stage(stage: &mut Stage, now: Instant) -> Option<Result<(), u8>> {
    let started = match stage.cleanup {
        Cleanup::None => return Some(Ok(())),
        Cleanup::Queued(started) | Cleanup::Pending { started, .. } => started,
    };
    // A lifecycle watchdog, not an audio wait budget. A broken helper cannot
    // keep a later start pending forever; control replaces it after failure.
    if now.saturating_duration_since(started) >= Duration::from_secs(5)
        || stage.instance.restart.load(Ordering::Acquire)
    {
        fail_cleanup(stage);
        return Some(Err(MISS_DEADLINE));
    }
    if matches!(stage.cleanup, Cleanup::Queued(_)) {
        match stage.host.submit_planes(1, &stage.input, &stage.routed) {
            Ok(generation) => {
                stage.cleanup = Cleanup::Pending {
                    generation,
                    started,
                }
            }
            Err(echode_sandbox::SessionError::Missed) => return None,
            Err(_) => {
                fail_cleanup(stage);
                return Some(Err(MISS_EXITED));
            }
        }
    }
    let Cleanup::Pending { generation, .. } = stage.cleanup else {
        return None;
    };
    match poll_stage(stage, generation, 1) {
        Poll::Pending => None,
        Poll::Ready => {
            stage.cleanup = Cleanup::None;
            stage.chase_pending = false;
            stage.chase_in_buffer = false;
            stage.block_complete = false;
            Some(Ok(()))
        }
        Poll::Failed(reason) => {
            fail_cleanup(stage);
            Some(Err(reason))
        }
    }
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
    let output = source.rendered();
    if dest.bypassed {
        crate::copy_bypassed_audio(
            output.iter().map(Vec::as_slice),
            &mut dest.bypass_output,
            frames,
        );
        return;
    }
    let main = output.len();
    let frames = frames
        .min(bounded(output, frames))
        .min(bounded(&dest.input, frames));
    let signal = Signal {
        left: &output[0],
        right: (main == 2).then(|| output[1].as_slice()),
    };
    for (channel, plane) in dest.input.iter_mut().enumerate() {
        for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
            *sample = if channel < dest.main_inputs {
                signal.sample(channel, frame, dest.main_inputs == 2)
            } else {
                0.0
            };
        }
    }
}

fn validate_chain(stages: &mut [Stage], clip_stereo: bool) -> Result<(), PlaybackError> {
    let mut channels = if clip_stereo { 2 } else { 1 };
    for (index, stage) in stages.iter_mut().enumerate() {
        let frames = stage.output.first().map_or(0, Vec::len);
        stage
            .bypass_output
            .resize_with(channels, || vec![0.0; frames]);
        for plane in &mut stage.bypass_output {
            plane.resize(frames, 0.0);
        }
        if stage.bypassed {
            continue;
        }
        // These are the layouts implemented by the stereo master, rather than
        // a storage/channel cap. Auxiliary ports retain their declared sizes.
        if !(1..=2).contains(&stage.main_outputs)
            || stage.main_inputs > 2
            || (channels == 2 && stage.main_inputs == 1 && !(index == 0 && stage.instrument))
            || (index == 0 && stage.instrument && clip_stereo && stage.main_outputs == 1)
        {
            return Err(PlaybackError::Unsupported(format!(
                "device {} ({}): unsupported mono/stereo bus mapping",
                stage.device, stage.clap_id
            )));
        }
        channels = stage.main_outputs;
    }
    Ok(())
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
