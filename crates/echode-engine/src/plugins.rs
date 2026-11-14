//! One helper process per CLAP device, prepared on the control thread.
//!
//! The callback submits every independent track, then collects them until one
//! deadline. Missed plugin output fades to silence and recovered output fades
//! back in. Main mono and stereo buses preserve their layout through the chain
//! and into the fader.
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
use crate::compensation::DeviceLatency;
use crate::delay::Delay;
use crate::plugin_state::{PluginState, holds};

/// Control-thread factory for a fresh send right on each live helper launch.
/// The captured owner retains the selected device's workgroup.
pub type PluginWorkgroup = Arc<
    dyn Fn() -> Result<echode_sandbox::WorkgroupHandoff, echode_sandbox::WorkgroupError>
        + Send
        + Sync,
>;
use crate::mix::Signal;

/// Where a CLAP id lives on disk, and its instrument role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginRoute {
    pub clap_id: String,
    pub path: PathBuf,
    pub instrument: bool,
    /// Passed to that helper as `ECHODE_TEST_FAULT`. Product routes leave this empty.
    pub fault: Option<String>,
    /// The CLAP state this device's helper should hold. A recorded state is
    /// read only when a helper loads it. `None` leaves the plugin's default.
    pub state: Option<PluginState>,
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
    report: BlockReport,
    cleanup_remaining: usize,
}

/// What a block's tracks tell the caller beyond their audio.
#[derive(Default)]
struct BlockReport {
    /// A block tried to send more events than the routed buffer can hold.
    dropped_events: bool,
    /// Device whose plugin missed this block. Zero reason means none.
    /// Live playback still fades out that track. Export reads the reason.
    export_reason: u8,
    /// Every stage that missed this block and why, for the log (ADR 0322).
    /// The control thread reserves a place for each stage, so the audio thread
    /// never grows it; a miss past that capacity is left out of the log only.
    misses: Vec<(u128, u8)>,
}

impl BlockReport {
    /// Names the first miss of the block, on `stage` when it is known, and
    /// keeps every miss on a known stage for the log.
    fn note_miss(&mut self, stage: Option<&Stage>, reason: u8) {
        if let Some(stage) = stage
            && self.misses.len() < self.misses.capacity()
        {
            self.misses.push((stage.device, reason));
        }
        if self.export_reason != 0 {
            return;
        }
        if let Some(stage) = stage {
            stage.instance.failure.store(reason, Ordering::Release);
        }
        self.export_reason = reason;
    }
}

pub(crate) const MISS_PROCESS: u8 = 1;
pub(crate) const MISS_DEADLINE: u8 = 2;
pub(crate) const MISS_SUBMIT: u8 = 3;
pub(crate) const MISS_EXITED: u8 = 4;
/// Not a miss: the plugin answered, but with a NaN or an infinity (ADR 0282).
const FAULT_NONFINITE: u8 = 5;

/// The first stage of a track that returned a NaN or an infinity this block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageFault {
    pub(crate) device: u128,
    /// Block frame of the first such sample. The stage's output is zero from here.
    pub(crate) frame: usize,
}

/// A track's collected plugin audio, as [`HostGraph::apply`] wrote it.
pub(crate) struct Applied {
    pub(crate) stereo: bool,
    pub(crate) fault: Option<StageFault>,
}

/// What [`HostGraph::submit`] did with a track's block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Submitted {
    /// No helper processes this track. The caller keeps its source.
    Unhosted,
    /// A helper is processing it. [`HostGraph::progress`] reports the end.
    Pending,
    /// The block already ended, heard through bypasses or missed at submission.
    Done,
}

/// What [`HostGraph::progress`] saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    /// At least one track finished, or none was pending.
    Finished,
    /// The deadline passed. Every pending track was abandoned and reported.
    Late,
}

/// One reading of every hosted device's latency and tail (ADR 0340).
pub(crate) struct Latencies {
    /// In graph order.
    devices: Vec<DeviceLatency>,
    /// `(key, from, to)` into `devices`, sorted by key.
    tracks: Vec<(u32, usize, usize)>,
}

impl Latencies {
    /// A track's devices in chain order. Empty when no helper runs it.
    pub(crate) fn of(&self, key: u32) -> &[DeviceLatency] {
        self.tracks
            .binary_search_by_key(&key, |entry| entry.0)
            .ok()
            .and_then(|index| self.tracks.get(index))
            .and_then(|&(_, from, to)| self.devices.get(from..to))
            .unwrap_or_default()
    }

    /// Each device's latency in graph order, to compare with later reports.
    pub(crate) fn reported(&self) -> Vec<(u128, u32)> {
        self.devices
            .iter()
            .map(|device| (device.device, device.latency))
            .collect()
    }
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
    inject_right: Vec<f32>,
    inject_stereo: bool,
    inject_live: bool,
    /// Gains for the first instrument's own output, applied before placed
    /// audio and children are added. Live only for a block whose track gates
    /// its own audio (ADR 0338).
    own: Vec<f32>,
    own_live: bool,
    /// This block's events, copied once so each stage can keep or skip note-ons.
    source_events: Vec<Event>,
    /// Note-ons retained helpers already hold. Only a new helper receives them.
    chased_ons: Vec<Event>,
    chase_order: Vec<usize>,
    chase_scratch: Vec<Event>,
    /// Set by the first stage of this block that returned a non-finite sample.
    fault: Option<StageFault>,
    /// The first instrument's output, delayed to meet the track's placed
    /// audio and children (ADR 0339). Empty in the control thread's graph.
    instrument_delay: Delay,
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
            by_key
                .windows(2)
                .all(|pair| matches!(pair, [left, right] if left.0 != right.0)),
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

    fn by_key(&self, key: u32) -> Option<&TrackHost> {
        self.list.get(self.position(key)?)
    }

    fn by_key_mut(&mut self, key: u32) -> Option<&mut TrackHost> {
        let position = self.position(key)?;
        self.list.get_mut(position)
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
    /// Device, bypass, and the dry and promoted plane counts to put back.
    devices: Vec<(u128, bool, usize, usize)>,
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
    /// Two planes when this mono first instrument starts a stereo track source.
    /// Its output enters both at equal power and placed audio keeps its own
    /// channels, so no stereo source is folded to mono (ADR 0095, ADR 0337).
    /// Empty for every other stage.
    promoted: Vec<Vec<f32>>,
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
        } else if !self.promoted.is_empty() {
            &self.promoted
        } else {
            self.output.get(..self.main_outputs).unwrap_or(&self.output)
        }
    }

    fn rendered_mut(&mut self) -> &mut [Vec<f32>] {
        if self.bypassed {
            &mut self.bypass_output
        } else if !self.promoted.is_empty() {
            &mut self.promoted
        } else {
            self.main_output_mut()
        }
    }

    /// What the device itself wrote, before placed audio and children join
    /// it: the mono plane a promotion reads, or the rendered planes.
    fn own_mut(&mut self) -> &mut [Vec<f32>] {
        if !self.bypassed && !self.promoted.is_empty() {
            let main = self.output.len().min(1);
            self.output.get_mut(..main).unwrap_or_default()
        } else {
            self.rendered_mut()
        }
    }

    /// The planes of the first output port, the track's audio.
    fn main_output_mut(&mut self) -> &mut [Vec<f32>] {
        let main = self.main_outputs.min(self.output.len());
        self.output.get_mut(..main).unwrap_or_default()
    }
}

/// Which of a stage's planes to check for a NaN or an infinity.
#[derive(Clone, Copy)]
enum Planes {
    /// What the device itself wrote.
    Own,
    /// What the next stage and the track read.
    Heard,
}

pub(crate) struct PreparationCheckpoint(Vec<(Arc<Instance>, bool)>);

pub(crate) struct ReconcileRoutes<'a> {
    pub entries: &'a [PluginRoute],
    pub restore_states: bool,
    /// Devices whose state matched at submission. Later live edits to those
    /// helpers must not turn an unrelated preparation into a stale reload.
    pub preserve: &'a [u128],
    /// Set by the preparation's owner to stop starting helpers. The helper
    /// starting then is stopped too, and reconciling fails with
    /// [`PlaybackError::Cancelled`] (ADR 0334).
    pub cancel: Option<&'a AtomicBool>,
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
            report: BlockReport::default(),
            cleanup_remaining: 0,
        }
    }

    pub(crate) fn dropped_events(&self) -> bool {
        self.report.dropped_events
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
        self.report.export_reason != 0
    }

    /// Each stage that missed the block just rendered, with its `MISS_*` code.
    pub(crate) fn misses(&self) -> &[(u128, u8)] {
        &self.report.misses
    }

    /// True when the helper hosting `device` has exited. Never waits: a host
    /// another control thread holds (a preparation, a state save) counts as
    /// running, so the log never stalls the document thread behind a helper.
    pub(crate) fn exited(&self, device: u128) -> bool {
        self.tracks
            .iter()
            .flat_map(|track| &track.stages)
            .find(|stage| stage.device == device)
            .and_then(|stage| stage.instance.host.try_lock().ok())
            .is_some_and(|mut host| host.has_exited())
    }

    pub(crate) fn begin_block(&mut self) {
        for stage in self.tracks.iter().flat_map(|track| &track.stages) {
            stage.instance.failure.store(0, Ordering::Release);
        }
        self.report.export_reason = 0;
        self.report.misses.clear();
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
            let Some(track) = event
                .track_key()
                .and_then(|key| self.tracks.by_key_mut(key))
            else {
                continue;
            };
            let held = &mut track.source_events;
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
            for stage in self.tracks.iter_mut().flat_map(|track| &mut track.stages) {
                if matches!(stage.cleanup, Cleanup::None) {
                    continue;
                }
                if let Some(result) = poll_cleanup_stage(stage, now) {
                    self.cleanup_remaining -= 1;
                    if let Err(reason) = result {
                        self.report.note_miss(Some(stage), reason);
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
                for stage in self.tracks.iter_mut().flat_map(|track| &mut track.stages) {
                    if matches!(stage.cleanup, Cleanup::None) {
                        continue;
                    }
                    fail_cleanup(stage);
                    self.cleanup_remaining -= 1;
                    self.report.note_miss(Some(stage), MISS_DEADLINE);
                }
                return true;
            }
            if let Some(fan) = &self.fan {
                let _ = fan.wait_until(seen, deadline);
                seen = fan.load();
            }
        }
    }

    /// Why a plugin did not produce this block, or produced NaN or infinity,
    /// including its device id. `None` when every submitted plugin finished
    /// cleanly. Live playback conceals a miss by a fade and auto-mutes a
    /// non-finite track; this is what export refuses to call success.
    pub(crate) fn export_failure(&self) -> Option<crate::PluginBlockFailure> {
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
            use crate::PluginBlockFailureReason;
            let reason = match reason {
                MISS_PROCESS => PluginBlockFailureReason::ProcessRefused,
                MISS_DEADLINE => PluginBlockFailureReason::Deadline,
                MISS_SUBMIT => PluginBlockFailureReason::Submit,
                FAULT_NONFINITE => PluginBlockFailureReason::NonFinite,
                _ => PluginBlockFailureReason::Exited,
            };
            return Some(crate::PluginBlockFailure {
                device: stage.device,
                clap_id: stage.clap_id.clone(),
                reason,
            });
        }
        None
    }

    /// Allocate an audio-owned bank using the already prepared process endpoints.
    /// `instrument_delays` gives each hosted track's first-instrument delay by
    /// key (ADR 0339).
    pub(crate) fn audio_snapshot(&self, mut instrument_delays: Vec<(u32, Delay)>) -> Self {
        instrument_delays.sort_unstable_by_key(|entry| entry.0);
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
                            promoted: stage.promoted.clone(),
                            routed: Vec::with_capacity(stage.routed.capacity()),
                        })
                        .collect();
                    let mut host = track_host(
                        track.key,
                        stages,
                        track.inject.len(),
                        track.source_events.capacity(),
                        track.chased_ons.capacity(),
                    );
                    if let Ok(index) =
                        instrument_delays.binary_search_by_key(&track.key, |entry| entry.0)
                        && let Some(entry) = instrument_delays.get_mut(index)
                    {
                        host.instrument_delay = std::mem::take(&mut entry.1);
                    }
                    host
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
        // One place per stage: a stage misses a block at most once.
        graph.report.misses = Vec::with_capacity(graph.stage_index.len());
        graph
    }

    /// Callback boundary only. Preserve DSP ownership and chase/reset receipts,
    /// and continue each track's compensation (ADR 0339).
    pub(crate) fn carry(&mut self, old: &mut Self) {
        for track in self.tracks.iter_mut() {
            if let Some(previous) = old.tracks.by_key_mut(track.key) {
                track.instrument_delay.carry(&mut previous.instrument_delay);
            }
        }
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
                    .map_err(|error| PlaybackError::helper(stage.device, error))?;
            }
        }
        Ok(())
    }

    /// Every hosted device's reported latency and declared tail, read once
    /// (ADR 0340). A plan's delays and the values a later restart is compared
    /// with both come from this one reading, so a helper that restarts while
    /// a plan is prepared is still noticed. Control only: reading a tail locks
    /// the helper.
    pub(crate) fn latencies(&self) -> Latencies {
        let mut devices = Vec::with_capacity(self.stage_index.len());
        let mut tracks = Vec::with_capacity(self.tracks.len());
        for track in self.tracks.iter() {
            let from = devices.len();
            devices.extend(track.stages.iter().map(|stage| DeviceLatency {
                device: stage.device,
                latency: stage.instance.control.latency_samples(),
                tail: stage.instance.host().tail_samples(),
                instrument: stage.instrument,
                bypassed: stage.bypassed,
            }));
            tracks.push((track.key, from, devices.len()));
        }
        tracks.sort_unstable_by_key(|entry| entry.0);
        Latencies { devices, tracks }
    }

    /// A helper reported a latency other than `reported`, which
    /// [`Latencies::reported`] returned for the same graph.
    pub(crate) fn latency_differs(&self, reported: &[(u128, u32)]) -> bool {
        let mut stages = self.tracks.iter().flat_map(|track| &track.stages);
        let mut reported = reported.iter();
        loop {
            match (stages.next(), reported.next()) {
                (None, None) => return false,
                (Some(stage), Some(&(device, latency)))
                    if stage.device == device
                        && stage.instance.control.latency_samples() == latency => {}
                _ => return true,
            }
        }
    }

    /// Silences every instrument's compensation at a discontinuity that also
    /// resets the plugins. O(1) per track.
    pub(crate) fn clear_delays(&mut self) {
        for track in self.tracks.iter_mut() {
            track.instrument_delay.clear();
        }
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
                holds(stage.instance.control.applied_state().as_ref(), incoming)
                    .then_some(device.id)
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
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, PlaybackError> {
        let mut graph = Self::empty();
        graph.reconcile(
            host_bin,
            ReconcileRoutes {
                entries: routes,
                restore_states: true,
                preserve: &[],
                cancel,
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
                    .map(|stage| {
                        (
                            stage.device,
                            stage.bypassed,
                            stage.bypass_output.len(),
                            stage.promoted.len(),
                        )
                    })
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
                        .and_then(|route| route.state.as_ref());
                    if selection.restore_states
                        && !selection.preserve.contains(&device.id)
                        && kept.get(index).is_some_and(|stage| {
                            !holds(stage.instance.control.applied_state().as_ref(), incoming)
                        })
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
                if selection
                    .cancel
                    .is_some_and(|flag| flag.load(Ordering::Acquire))
                {
                    self.tracks = HostTracks::new(restore_tracks(restore, tracks, stages, kept));
                    return Err(PlaybackError::Cancelled);
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
                        cancel: selection.cancel,
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
    /// `source` contains placed audio. `clips` is the placed audio alone,
    /// with the children's bus on a parent. The first instrument ignores
    /// `source` and receives notes; `clips` is added to that instrument's
    /// output. `own`, when given, scales that instrument's output first. A track
    /// with no helper is left idle so the caller keeps `source`. `events` and
    /// `chased_ons` are this track's alone; the caller groups them once.
    pub(crate) fn submit(
        &mut self,
        key: u32,
        source: Signal<'_>,
        clips: Signal<'_>,
        events: &[Event],
        chased_ons: &[Event],
        own: Option<&[f32]>,
    ) -> Submitted {
        let Some(track) = self.tracks.by_key_mut(key) else {
            return Submitted::Unhosted;
        };
        track.heard = false;
        track.stage = 0;
        track.run = Run::Idle;
        track.fault = None;
        if track.stages.is_empty() || source.left.is_empty() {
            return Submitted::Unhosted;
        }
        let frames = source.left.len();
        let report = &mut self.report;
        track.remember(report, events, chased_ons);
        track.arm_inject(clips, frames);
        track.arm_own(own, frames);
        track.prepare_input(report, 0, source, frames);
        if !track.submit_stage(report, 0, frames) {
            track.miss(report, MISS_SUBMIT);
            track.frames = frames;
        }
        if track.run == Run::Pending {
            Submitted::Pending
        } else {
            Submitted::Done
        }
    }

    /// The fan counter to pass to the first [`HostGraph::progress`] of a block.
    pub(crate) fn fan_seen(&self) -> u64 {
        self.fan.as_ref().map(|fan| fan.load()).unwrap_or(0)
    }

    /// Waits until a submitted track finishes its block or `deadline` passes.
    ///
    /// Each track whose block ends here, heard or missed, is pushed to
    /// `finished` once. A helper that finished a stage is kept, and its next
    /// stage is submitted and polled here. At the deadline every pending track
    /// is abandoned and pushed. `seen` is the fan counter the caller last saw.
    pub(crate) fn progress(
        &mut self,
        finished: &mut Vec<u32>,
        seen: &mut u64,
        deadline: Instant,
    ) -> Progress {
        loop {
            self.poll_pass(Some(&mut *finished));
            if !finished.is_empty() || !self.tracks.iter().any(|track| track.run == Run::Pending) {
                return Progress::Finished;
            }
            if Instant::now() >= deadline {
                self.abandon_pending(Some(finished));
                return Progress::Late;
            }
            let Some(fan) = &self.fan else {
                self.abandon_pending(Some(finished));
                return Progress::Late;
            };
            let _ = fan.wait_until(*seen, deadline);
            *seen = fan.load();
        }
    }

    /// Gives up on a track submitted after the block's deadline (ADR 0338).
    /// Its helper still receives the events, so no note is lost, but the
    /// block is missed like any other late answer.
    pub(crate) fn abandon_track(&mut self, key: u32) {
        let report = &mut self.report;
        if let Some(track) = self.tracks.by_key_mut(key)
            && track.run == Run::Pending
        {
            track.abandon(report);
        }
    }

    /// Collects every track submitted for this callback.
    ///
    /// `deadline` is the one absolute time for the callback. A helper that is
    /// still running at that time is abandoned. A helper that already finished
    /// is kept, and its next stage is submitted and polled in this same collect.
    /// Leaving that stage pending would keep the dry track audio.
    pub(crate) fn collect(&mut self, deadline: Instant) {
        let mut seen = self.fan_seen();
        loop {
            self.poll_pass(None);
            // `advance` may have submitted the next stage. That stage is pending
            // even though this pass did not observe it waiting.
            if !self.tracks.iter().any(|track| track.run == Run::Pending) {
                return;
            }
            if Instant::now() >= deadline {
                self.abandon_pending(None);
                return;
            }
            let Some(fan) = &self.fan else {
                self.abandon_pending(None);
                return;
            };
            let _ = fan.wait_until(seen, deadline);
            seen = fan.load();
        }
    }

    /// Polls every pending track once. A track whose block ended in this pass
    /// is pushed to `finished`, when given.
    fn poll_pass(&mut self, mut finished: Option<&mut Vec<u32>>) {
        for track in self.tracks.iter_mut() {
            if track.run != Run::Pending {
                continue;
            }
            match track.poll() {
                Poll::Ready => track.advance(&mut self.report),
                Poll::Failed(reason) => {
                    if let Some(stage) = track.stages.get_mut(track.stage)
                        && stage.chase_pending
                    {
                        stage.chase_pending = false;
                        stage.needs_restart = true;
                        stage.instance.restart.store(true, Ordering::Release);
                    }
                    track.miss(&mut self.report, reason);
                }
                Poll::Pending => continue,
            }
            if track.run != Run::Pending
                && let Some(finished) = finished.as_deref_mut()
            {
                push_finished(finished, track.key, &mut self.report);
            }
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

    /// Writes collected plugin audio back, concealing misses before the fader.
    ///
    /// A track that is `muted`, or whose stage returned a non-finite sample
    /// this block, fades out like a miss from that sample on (ADR 0282).
    pub(crate) fn apply(
        &self,
        key: u32,
        left: &mut [f32],
        right: &mut [f32],
        declick: &mut crate::PluginDeclick,
        muted: bool,
    ) -> Option<Applied> {
        let track = self.tracks.by_key(key)?;
        let frames = left.len().min(right.len()).min(track.frames);
        let left = left.get_mut(..frames)?;
        let right = right.get_mut(..frames)?;
        // A failed intermediate mono stage must not change the final bus shape.
        let stereo = track.stages.last()?.rendered().len() == 2;
        match track.run {
            Run::Heard => {
                let output = track.stages.get(track.stage)?.rendered();
                let from_left = output.first()?.get(..frames)?;
                let from_right = output.get(usize::from(stereo))?.get(..frames)?;
                left.copy_from_slice(from_left);
                right.copy_from_slice(from_right);
            }
            Run::Missed => {}
            Run::Idle | Run::Pending => return None,
        }
        let silent = if muted {
            0
        } else {
            track.fault.map_or(frames, |fault| fault.frame.min(frames))
        };
        let (left_heard, left_faded) = left.split_at_mut_checked(silent)?;
        let (right_heard, right_faded) = right.split_at_mut_checked(silent)?;
        declick.process(left_heard, right_heard, track.run == Run::Missed);
        declick.process(left_faded, right_faded, true);
        Some(Applied {
            stereo,
            fault: track.fault,
        })
    }

    /// Starts every plugin of the track at `key` afresh before its next block,
    /// so the state that produced a fault does not repeat it.
    pub(crate) fn reset_track(&mut self, key: u32) {
        if let Some(track) = self.tracks.by_key_mut(key) {
            for stage in &mut track.stages {
                stage.host.request_reset();
            }
        }
    }

    fn abandon_pending(&mut self, mut finished: Option<&mut Vec<u32>>) {
        for track in self.tracks.iter_mut() {
            if track.run != Run::Pending {
                continue;
            }
            track.abandon(&mut self.report);
            if let Some(finished) = finished.as_deref_mut() {
                push_finished(finished, track.key, &mut self.report);
            }
        }
    }
}

/// The caller reserves one place per planned track and clears the list
/// before each pass, and a track ends once per block, so this never grows
/// the vector. A full list is reported as lost events rather than allocating.
fn push_finished(finished: &mut Vec<u32>, key: u32, report: &mut BlockReport) {
    if finished.len() < finished.capacity() {
        finished.push(key);
    } else {
        report.dropped_events = true;
    }
}

/// One track's block, from its first stage to its last. The callback looks a
/// track up once; these methods reach its stages by `get`, so a stage index
/// that does not exist misses the block instead of panicking (ADR 0284).
impl TrackHost {
    /// Marks this block missed at the current stage and names it for export.
    fn miss(&mut self, report: &mut BlockReport, reason: u8) {
        report.note_miss(self.stages.get(self.stage), reason);
        self.run = Run::Missed;
    }

    /// Stops waiting for the stage in flight. A plugin that already refused
    /// or exited is not a deadline miss, even after the block's time is up.
    fn abandon(&mut self, report: &mut BlockReport) {
        if let Poll::Failed(reason) = self.poll() {
            self.miss(report, reason);
            return;
        }
        let generation = self.generation;
        if let Some(stage) = self.stages.get_mut(self.stage) {
            stage.host.abandon(generation);
        }
        self.miss(report, MISS_DEADLINE);
    }

    /// Keeps this block's gains for the first instrument's own output.
    fn arm_own(&mut self, gains: Option<&[f32]>, frames: usize) {
        let instrument = self.stages.first().is_some_and(|stage| stage.instrument);
        self.own_live = false;
        let Some(gains) = gains.filter(|_| instrument) else {
            return;
        };
        let frames = frames.min(self.own.len());
        for (gain, source) in self.own.iter_mut().take(frames).zip(gains) {
            *gain = *source;
        }
        self.own_live = true;
    }

    /// Polls the stage in flight. A stage that does not exist cannot answer.
    fn poll(&mut self) -> Poll {
        let (generation, frames) = (self.generation, self.frames);
        match self.stages.get_mut(self.stage) {
            Some(stage) => poll_stage(stage, generation, frames),
            None => Poll::Failed(MISS_SUBMIT),
        }
    }

    fn remember(&mut self, report: &mut BlockReport, events: &[Event], chased_ons: &[Event]) {
        let key = self.key;
        let mut dropped = copy_events(&mut self.source_events, events, |_| true);
        self.chased_ons.clear();
        dropped |= copy_events(&mut self.chased_ons, chased_ons, |event| {
            event.track_key() == Some(key)
        });
        if !self.chased_ons.is_empty()
            && !sort_events(
                &mut self.chased_ons,
                &mut self.chase_order,
                &mut self.chase_scratch,
            )
        {
            dropped = true;
        }
        report.dropped_events |= dropped;
    }

    fn prepare_input(
        &mut self,
        report: &mut BlockReport,
        stage_index: usize,
        source: Signal<'_>,
        frames: usize,
    ) {
        let heard = self.heard;
        let Some(stage) = self.stages.get_mut(stage_index) else {
            return;
        };
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
                plane
                    .iter_mut()
                    .take(frames)
                    .for_each(|sample| *sample = 0.0);
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
        self.route_events(report, stage_index);
    }

    /// A helper that already received a sounding note does not get that note-on
    /// again. A helper spawned while the note is held still does.
    fn route_events(&mut self, report: &mut BlockReport, stage_index: usize) {
        let TrackHost {
            source_events,
            chased_ons,
            stages,
            ..
        } = self;
        let Some(stage) = stages.get_mut(stage_index) else {
            return;
        };
        if stage.bypassed {
            return;
        }
        let include_chase = stage.awaiting_notes && !stage.needs_restart && !stage.chase_pending;
        stage.routed.clear();
        let chase = if include_chase {
            chased_ons.as_slice()
        } else {
            &[]
        };
        let mut dropped = source_events.len() + chase.len() > stage.routed.capacity();
        let mut sources = source_events.iter().peekable();
        let mut chased = chase.iter().peekable();
        loop {
            let take_chase = match (sources.peek(), chased.peek()) {
                (None, None) => break,
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some(source), Some(chased)) => {
                    event_cmp(chased, source) != std::cmp::Ordering::Greater
                }
            };
            if stage.routed.len() == stage.routed.capacity() {
                dropped = true;
                break;
            }
            let next = if take_chase {
                chased.next()
            } else {
                sources.next()
            };
            let Some(event) = next else {
                break;
            };
            stage.routed.push((*event, 0));
        }
        stage.chase_in_buffer = include_chase && !dropped && !chase.is_empty();
        // A note-on that starts in this block is ordinary source, not chase.
        // Once that block is heard, the helper already has the voice, so a
        // later chase must not play it again.
        stage.block_complete = !dropped && (include_chase || !stage.awaiting_notes);
        report.dropped_events |= dropped;
    }

    fn submit_stage(
        &mut self,
        report: &mut BlockReport,
        mut stage_index: usize,
        mut frames: usize,
    ) -> bool {
        // A run of bypassed stages consumes no helper deadline and no stack
        // frames. The number of devices is not limited by recursive calls.
        loop {
            self.stage = stage_index;
            let Some(stage) = self.stages.get_mut(stage_index) else {
                return false;
            };
            if !stage.bypassed {
                break;
            }
            frames = bounded(&stage.bypass_output, frames);
            stage.awaiting_notes = true;
            self.frames = frames;
            self.mix_injected(stage_index, frames);
            self.heard = true;
            let next = stage_index + 1;
            if next == self.stages.len() {
                self.run = Run::Heard;
                return true;
            }
            copy_mix(&mut self.stages, stage_index, next, frames);
            self.route_events(report, next);
            stage_index = next;
        }
        let Some(stage) = self.stages.get_mut(stage_index) else {
            return false;
        };
        let frames = bounded(&stage.input, frames.min(bounded(&stage.output, frames)));
        let generation = stage
            .host
            .submit_planes(frames, &stage.input, &stage.routed);
        match generation {
            Ok(generation) => {
                if stage.chase_in_buffer {
                    stage.chase_pending = true;
                }
                stage.chase_in_buffer = false;
                self.generation = generation;
                self.frames = frames;
                self.stage = stage_index;
                self.run = Run::Pending;
                true
            }
            Err(_) => {
                stage.chase_in_buffer = false;
                false
            }
        }
    }

    fn arm_inject(&mut self, clips: Signal<'_>, frames: usize) {
        let instrument = self.stages.first().is_some_and(|stage| stage.instrument);
        if !instrument {
            self.inject_live = false;
            return;
        }
        let frames = frames.min(self.inject.len());
        for (frame, (left, right)) in self
            .inject
            .iter_mut()
            .zip(&mut self.inject_right)
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
        self.inject_stereo = clips.right.is_some();
        self.inject_live = true;
    }

    fn advance(&mut self, report: &mut BlockReport) {
        let frames = self.frames;
        let stage_index = self.stage;
        // The device's own output first: a delay line is memory, and a NaN
        // kept there would outlive the reset that releases the auto-mute
        // (NFA-07). Then the sum with the placed audio and the children.
        self.contain_nonfinite(stage_index, frames, Planes::Own);
        if self.mix_injected(stage_index, frames) {
            self.contain_nonfinite(stage_index, frames, Planes::Heard);
        }
        if let Some(stage) = self.stages.get_mut(stage_index) {
            if stage.block_complete {
                stage.awaiting_notes = false;
            }
            stage.chase_pending = false;
            stage.block_complete = false;
        }
        let next = stage_index + 1;
        self.heard = true;
        if next >= self.stages.len() {
            self.run = Run::Heard;
            return;
        }
        if self.hand_off(report, stage_index, next, frames) {
            return;
        }
        self.miss(report, MISS_SUBMIT);
    }

    /// Zeroes a stage's output from its first NaN or infinity, so the next
    /// plugin, the track and the delay lines never receive one, and names the
    /// stage once.
    fn contain_nonfinite(&mut self, stage_index: usize, frames: usize, planes: Planes) {
        let Some(stage) = self.stages.get_mut(stage_index) else {
            return;
        };
        if stage.bypassed {
            return;
        }
        let output = match planes {
            Planes::Own => stage.own_mut(),
            // The planes the next stage and the track read, promoted or not.
            Planes::Heard => stage.rendered_mut(),
        };
        let frames = bounded(output, frames);
        let Some(first) = output
            .iter()
            .filter_map(|plane| {
                plane
                    .iter()
                    .take(frames)
                    .position(|sample| !sample.is_finite())
            })
            .min()
        else {
            return;
        };
        for plane in output.iter_mut() {
            if let Some(tail) = plane.get_mut(first..frames) {
                tail.fill(0.0);
            }
        }
        // Export refuses the block; live playback auto-mutes the track.
        stage
            .instance
            .failure
            .store(FAULT_NONFINITE, Ordering::Release);
        if self.fault.is_none() {
            self.fault = Some(StageFault {
                device: stage.device,
                frame: first,
            });
        }
    }

    /// Adds placed audio onto the first instrument, once, before the next effect
    /// or the track output reads that buffer.
    ///
    /// A promoted mono instrument is rewritten from its own output every
    /// block, so its planes never hold a previous block's audio.
    ///
    /// Returns whether the stage is an instrument, whose planes this may have
    /// changed.
    fn mix_injected(&mut self, stage_index: usize, frames: usize) -> bool {
        let TrackHost {
            stages,
            inject,
            inject_right,
            inject_stereo,
            inject_live,
            own,
            own_live,
            instrument_delay,
            ..
        } = self;
        let Some(stage) = stages.get_mut(stage_index) else {
            return false;
        };
        if !stage.instrument {
            return false;
        }
        if stage_index == 0 {
            delay_instrument(stage, instrument_delay, frames);
        }
        let live = std::mem::replace(inject_live, false);
        // The track's own instrument, before its clips and children join it.
        let gate = std::mem::replace(own_live, false).then_some(own.as_slice());
        let gain = |frame: usize| gate.map_or(1.0, |gate| gate.get(frame).copied().unwrap_or(1.0));
        if !stage.bypassed && !stage.promoted.is_empty() {
            let silence: &[f32] = &[];
            let source = if live {
                Signal {
                    left: inject,
                    right: inject_stereo.then_some(&*inject_right),
                }
            } else {
                Signal {
                    left: silence,
                    right: None,
                }
            };
            let Stage {
                output, promoted, ..
            } = stage;
            let mono = output.first().map_or(silence, Vec::as_slice);
            let frames = bounded(promoted, frames);
            for (channel, plane) in promoted.iter_mut().enumerate() {
                for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                    let dry = mono.get(frame).copied().unwrap_or(0.0) * gain(frame);
                    *sample =
                        dry * std::f32::consts::FRAC_1_SQRT_2 + source.sample(channel, frame, true);
                }
            }
            return true;
        }
        let output = stage.rendered_mut();
        if gate.is_some() {
            for plane in output.iter_mut() {
                for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                    *sample *= gain(frame);
                }
            }
        }
        if !live {
            return true;
        }
        let main = output.len();
        let frames = bounded(output, frames.min(inject.len()));
        let source = Signal {
            left: inject,
            right: inject_stereo.then_some(&*inject_right),
        };
        for (channel, plane) in output.iter_mut().enumerate() {
            for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                *sample += source.sample(channel, frame, main == 2);
            }
        }
        true
    }

    fn hand_off(
        &mut self,
        report: &mut BlockReport,
        from: usize,
        to: usize,
        frames: usize,
    ) -> bool {
        copy_mix(&mut self.stages, from, to, frames);
        self.route_events(report, to);
        self.submit_stage(report, to, frames)
    }
}

/// How long one block of `frames` lasts at `sample_rate`.
pub fn block_duration(frames: usize, sample_rate: u32) -> Duration {
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
        own: vec![1.0; frames],
        own_live: false,
        source_events: Vec::with_capacity(events.max(1)),
        chased_ons: Vec::with_capacity(chase.max(1)),
        chase_order: vec![0; chase.max(1)],
        chase_scratch: vec![Event::note_off(0, 0, 0, 0); chase.max(1)],
        fault: None,
        instrument_delay: Delay::default(),
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
                .filter_map(|(id, bypassed, channels, promoted)| {
                    // A restored state may have spawned another helper with the same
                    // device id. `kept` starts with the original process in that case.
                    let at = kept.iter().position(|stage| stage.device == id)?;
                    let mut stage = kept.remove(at);
                    stage.bypassed = bypassed;
                    stage
                        .bypass_output
                        .resize_with(channels, || vec![0.0; old.frames]);
                    stage
                        .promoted
                        .resize_with(promoted, || vec![0.0; old.frames]);
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
        .map_err(|error| PlaybackError::helper(stage.device, error))?;
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
    cancel: Option<&'a AtomicBool>,
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
        cancel,
    } = size;
    let Some(route) = take_route(routes, used, &device.clap_id) else {
        return Ok(None);
    };
    if !route.path.exists() {
        return Ok(None);
    }
    // Read before starting a process that could not be given its state.
    let state = route
        .state
        .as_ref()
        .map(PluginState::load)
        .transpose()
        .map_err(|error| PlaybackError::State {
            device: device.id,
            error,
        })?;
    let shared = if let Some(fan) = fan {
        Arc::clone(fan)
    } else {
        let created =
            Arc::new(FanIn::create().map_err(|error| PlaybackError::FanIn(Arc::new(error)))?);
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
        workgroup: workgroup.map(|source| source()).transpose()?,
        layout,
        fan_in: Some(shared),
        sample_rate,
        plugin_id: Some(route.clap_id.as_str()),
        cancel,
    }) {
        // Stopped as it started; dropping it ends the process.
        Ok(_) if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) => {
            return Err(PlaybackError::Cancelled);
        }
        Ok(host) => {
            if let Some(state) = &state {
                host.control_handle()
                    .restore_applied(state)
                    .map_err(|error| PlaybackError::helper(device.id, error))?;
            }
            host
        }
        Err(error) if error.is_cancelled() => return Err(PlaybackError::Cancelled),
        Err(error) => {
            if route.path.exists() {
                return Err(PlaybackError::Spawn {
                    device: device.id,
                    clap_id: device.clap_id.clone(),
                    error: Arc::new(error),
                });
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
        promoted: Vec::new(),
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

/// Delays the first instrument's own output, before its gate and before the
/// placed audio and children join it (ADR 0339). A promoted mono instrument
/// is delayed in its mono plane, which the promotion then reads.
fn delay_instrument(stage: &mut Stage, delay: &mut Delay, frames: usize) {
    if delay.frames() == 0 {
        return;
    }
    let planes = stage.own_mut();
    let frames = bounded(planes, frames);
    match planes {
        [left, right, ..] => {
            if let (Some(left), Some(right)) = (left.get_mut(..frames), right.get_mut(..frames)) {
                delay.process(left, Some(right));
            }
        }
        [left] => {
            if let Some(left) = left.get_mut(..frames) {
                delay.process(left, None);
            }
        }
        [] => {}
    }
}

fn copy_mix(stages: &mut [Stage], from: usize, to: usize, frames: usize) {
    let Some((head, tail)) = stages.split_at_mut_checked(to) else {
        return;
    };
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
    // A source without planes is silence, as in a bypass.
    let signal = Signal {
        left: output.first().map_or(&[], Vec::as_slice),
        right: output.get(1).filter(|_| main == 2).map(Vec::as_slice),
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
        // A mono first instrument on a stereo source enters the stereo bus
        // at equal power, like any mono source (ADR 0095, ADR 0337).
        let promote = index == 0 && stage.instrument && clip_stereo && stage.main_outputs == 1;
        stage
            .promoted
            .resize_with(if promote { 2 } else { 0 }, || vec![0.0; frames]);
        for plane in &mut stage.promoted {
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
        {
            return Err(PlaybackError::BusMapping {
                device: stage.device,
                clap_id: stage.clap_id.clone(),
            });
        }
        channels = if promote { 2 } else { stage.main_outputs };
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
