use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_engine::ClipLoader;
use echode_model::Project;
use serde::{Deserialize, Serialize};

use crate::opening::{Basis, HalOutputs, Opening, Outputs};
use crate::{
    Inventory, OutputDevice, OutputStream, PluginActivity, PluginControl, TransportCancellation,
    hardware,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum AudioCommand {
    #[serde(rename = "transport.play")]
    Play,
    #[serde(rename = "transport.pause")]
    Pause,
    #[serde(rename = "transport.stop")]
    Stop,
    #[serde(rename = "transport.locate")]
    Locate { tick: i64 },
    /// Remembers `tick` as where playback starts and stop returns, saves it
    /// with the package and moves the playhead there. A running transport
    /// seeks and keeps playing (ADR 0155).
    #[serde(rename = "transport.set_start")]
    SetStart { tick: i64 },
    #[serde(rename = "audio.output")]
    Output { uid: Option<String> },
    #[serde(rename = "audio.refresh")]
    Refresh,
    #[serde(rename = "audio.status")]
    Status,
    /// Opens the output stopped, helpers and media included, so the first play
    /// need not wait for them. The app sends it when a project opens (ADR 0147).
    #[serde(rename = "audio.prepare")]
    Prepare,
    #[serde(rename = "audio.preview_fader")]
    PreviewFader {
        track: echode_model::Id,
        gain: f32,
        pan: f32,
    },
    #[serde(rename = "audio.clear_fader_preview")]
    ClearFaderPreview,
    /// Both ticks set the loop. Both absent clear it. A tick is a project position.
    #[serde(rename = "transport.loop")]
    Loop {
        start: Option<i64>,
        end: Option<i64>,
    },
    #[serde(rename = "transport.metronome")]
    Metronome { enabled: bool },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AudioState {
    pub playing: bool,
    pub running: bool,
    pub tick: i64,
    /// The remembered start. Stop returns here; it persists with the package.
    pub start: i64,
    /// A pause holds the playhead; play resumes from it. Stop and a new
    /// start clear it.
    pub paused: bool,
    pub selected_uid: Option<String>,
    pub active_device: Option<OutputDevice>,
    pub devices: Vec<OutputDevice>,
    pub error: Option<String>,
    pub loop_start: Option<i64>,
    pub loop_end: Option<i64>,
    pub metronome: bool,
    /// Counters belong to the current retained output stream, including pauses.
    pub plugin_missed_blocks: u64,
    pub graph_unavailable_blocks: u64,
    pub pending_plan: bool,
    /// The sample audition, which runs beside this session's output.
    pub audition: Option<crate::AuditionReport>,
    /// Helpers and media are being prepared for the output on another thread.
    /// The worker keeps taking edits; nothing sounds until this clears.
    pub opening: bool,
}

/// A device view's way to its running plugin.
pub enum DeviceAccess {
    Ready(PluginControl),
    /// Initial output or replacement helpers are being prepared. Ask
    /// [`Session::opened_device_control`] once [`Session::opening`] is false;
    /// asking again does not open twice.
    Opening,
}

/// One foreground project. No hardware is queried until the app asks for the
/// output list, prepares the output or starts playback; package editing stays
/// fully headless.
pub struct Session {
    project: Arc<Project>,
    stream: Option<OutputStream>,
    /// Prepares off this thread. Its result becomes `stream` in `service`.
    opening: Option<Opening>,
    /// Abandoned openings, dropped once their threads exit.
    retiring: Vec<Opening>,
    outputs: Arc<dyn Outputs>,
    /// The inventory has been read at least once.
    refreshed: bool,
    notifications: Option<hardware::Notifications>,
    hardware_epoch: Arc<AtomicU64>,
    observed_epoch: u64,
    inventory: Inventory,
    selected_uid: Option<String>,
    tick: i64,
    /// Where stop returns, in project ticks. Saved in the package, not the model.
    start: i64,
    paused: bool,
    playing: bool,
    cancellation: TransportCancellation,
    play_revision: u64,
    loop_ticks: Option<(i64, i64)>,
    metronome: bool,
    clips: ClipLoader,
    package: Option<PathBuf>,
    error: Option<String>,
    fader_preview: Option<(u128, f32, f32)>,
    activity: Option<crate::activity::UserActivity>,
    /// A device view asked for running plugins, or the app prepared the output.
    /// The output stays open while stopped, as it does after playback.
    engine_held: bool,
    /// Play waits for media a stopped edit is loading: the plan it replaces
    /// must not sound in its place.
    resume_held: bool,
    /// Follows this session's output choice. Its requests do not come here.
    audition: Option<crate::Audition>,
    /// Devices whose helpers hold a change the document has not recorded yet.
    live_plugin_edits: Vec<u128>,
}

impl Session {
    pub fn new(project: Arc<Project>) -> Self {
        Self {
            project,
            stream: None,
            opening: None,
            retiring: Vec::new(),
            outputs: Arc::new(HalOutputs),
            refreshed: false,
            notifications: None,
            hardware_epoch: Arc::new(AtomicU64::new(0)),
            observed_epoch: 0,
            inventory: Inventory::default(),
            selected_uid: None,
            tick: 0,
            start: 0,
            paused: false,
            playing: false,
            cancellation: TransportCancellation::default(),
            play_revision: 0,
            loop_ticks: None,
            metronome: false,
            clips: Arc::new(|_project, _rate| Ok(Vec::new())),
            package: None,
            error: None,
            fader_preview: None,
            activity: None,
            engine_held: false,
            resume_held: false,
            audition: None,
            live_plugin_edits: Vec::new(),
        }
    }

    /// The document's audition plays on the output this session selects.
    pub fn set_audition(&mut self, audition: crate::Audition) {
        audition.follow_output(self.selected_uid.clone());
        self.audition = Some(audition);
    }

    /// Uses a stand-in output with no device. Preparation is unchanged.
    #[cfg(any(test, feature = "headless-output"))]
    pub fn use_headless_output(&mut self, output: crate::HeadlessOutput) {
        self.outputs = Arc::new(output);
    }

    /// Playback reads audio objects from `package` and starts at its
    /// remembered start.
    pub fn with_media(project: Arc<Project>, package: &Path) -> Self {
        let mut session = Self::new(project);
        session.clips = crate::clips::media_loader(package.to_path_buf());
        session.package = Some(package.to_path_buf());
        session.start = echode_history::read_transport_settings(package).start;
        session.tick = session.start;
        session
    }

    pub fn with_media_and_cancellation(
        project: Arc<Project>,
        package: &Path,
        cancellation: TransportCancellation,
    ) -> Self {
        let mut session = Self::with_media(project, package);
        session.cancellation = cancellation;
        session
    }

    pub fn perform(&mut self, command: AudioCommand) -> Result<(), String> {
        let revision = self.cancellation.request(&command);
        self.perform_requested(command, revision)
    }

    /// `revision` is captured when the caller queues the command, not when
    /// this worker finally reaches it after a potentially slow plugin load.
    pub fn perform_requested(
        &mut self,
        command: AudioCommand,
        revision: u64,
    ) -> Result<(), String> {
        match command {
            AudioCommand::PreviewFader { track, gain, pan } => {
                if !self.project.tracks().iter().any(|entry| entry.id == track)
                    || !gain.is_finite()
                    || !(0.0..=4.0).contains(&gain)
                    || !pan.is_finite()
                    || !(-1.0..=1.0).contains(&pan)
                {
                    return Err("invalid mixer preview".into());
                }
                if let Some(stream) = &mut self.stream {
                    stream.preview_fader(track.raw(), gain, pan)?;
                }
                self.fader_preview = Some((track.raw(), gain, pan));
            }
            AudioCommand::ClearFaderPreview => {
                if let Some(stream) = &mut self.stream {
                    stream.clear_fader_preview(false)?;
                }
                self.fader_preview = None;
            }
            AudioCommand::Play => {
                if !self.cancellation.is_current(revision) {
                    return Ok(());
                }
                let already_playing = self.playing
                    && self.play_revision == revision
                    && (self.stream.is_some() || self.opening.is_some());
                self.error = None;
                if !self.refreshed
                    || self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
                {
                    self.refresh()?;
                }
                self.play_revision = revision;
                self.playing = true;
                self.paused = false;
                if let Err(error) = self.reconcile() {
                    self.playing = false;
                    self.sync_activity();
                    self.error = Some(error.clone());
                    return Err(error);
                }
                self.playing = self.cancellation.is_current(revision);
                // An opening output starts at install, from the revision then current.
                if self.playing
                    && !already_playing
                    && let Err(error) = self.resume()
                {
                    self.cancellation.request(&AudioCommand::Stop);
                    self.playing = false;
                    self.sync_activity();
                    self.error = Some(error.clone());
                    return Err(error);
                }
            }
            AudioCommand::Pause | AudioCommand::Stop => {
                // Retain prepared media, helpers and HAL. Cancellation already
                // stopped the score without waiting for this queue; plugin
                // releases and autonomous output keep running in the callback.
                let stop = matches!(command, AudioCommand::Stop);
                self.paused = !stop && (self.playing || self.paused);
                self.playing = false;
                self.resume_held = false;
                if let Some(stream) = &mut self.stream {
                    self.tick = stream.status().tick;
                    stream.pause()?;
                    if stop {
                        // Only the clock returns. Tails keep sounding.
                        stream.rewind(&self.project, self.start)?;
                    }
                }
                if stop {
                    self.tick = self.start;
                }
            }
            AudioCommand::Locate { tick } => {
                if tick < 0 {
                    return Err("playback position must not be negative".into());
                }
                if let Some(stream) = &mut self.stream {
                    stream.locate(&self.project, tick)?;
                }
                self.tick = tick;
            }
            AudioCommand::SetStart { tick } => {
                if tick < 0 {
                    return Err("playback start must not be negative".into());
                }
                if let Some(package) = &self.package {
                    echode_history::write_transport_settings(
                        package,
                        echode_history::TransportSettings { start: tick },
                    )
                    .map_err(|error| error.to_string())?;
                }
                self.start = tick;
                self.paused = false;
                if let Some(stream) = &mut self.stream {
                    stream.locate(&self.project, tick)?;
                }
                self.tick = tick;
            }
            AudioCommand::Output { uid } => {
                self.refresh()?;
                if let Some(audition) = &self.audition {
                    audition.follow_output(uid.clone());
                }
                self.selected_uid = uid;
                self.error = None;
                self.reconcile()?;
            }
            AudioCommand::Refresh => {
                self.refresh()?;
                self.reconcile()?;
            }
            AudioCommand::Status => {}
            AudioCommand::Prepare => {
                self.engine_held = true;
                if self.stream.is_none() && self.opening.is_none() {
                    if !self.refreshed
                        || self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
                    {
                        self.refresh()?;
                    }
                    self.error = None;
                    self.reconcile()?;
                }
            }
            AudioCommand::Loop { start, end } => self.set_loop(start, end)?,
            AudioCommand::Metronome { enabled } => {
                self.metronome = enabled;
                if let Some(stream) = &mut self.stream {
                    stream.set_metronome(enabled)?;
                }
            }
        }
        self.sync_activity();
        Ok(())
    }

    /// Starts the open output's transport at `play_revision`, or holds the
    /// start until media for a stopped edit is queued. An opening output
    /// starts when it is installed.
    fn resume(&mut self) -> Result<(), String> {
        let Some(stream) = &mut self.stream else {
            return Ok(());
        };
        self.resume_held = stream.loading_media();
        if self.resume_held {
            return Ok(());
        }
        stream.resume(self.play_revision)
    }

    fn sync_activity(&mut self) {
        if self.playing {
            if self.activity.is_none() {
                self.activity = Some(crate::activity::UserActivity::begin("Echode is playing"));
            }
        } else {
            self.activity = None;
        }
    }

    fn set_loop(&mut self, start: Option<i64>, end: Option<i64>) -> Result<(), String> {
        match (start, end) {
            (Some(start), Some(end)) if start >= 0 && end > start => {
                if let Some(stream) = &mut self.stream {
                    stream.set_loop(&self.project, start, end)?;
                }
                self.loop_ticks = Some((start, end));
            }
            (None, None) => {
                if let Some(stream) = &mut self.stream {
                    stream.clear_loop()?;
                }
                self.loop_ticks = None;
            }
            _ => return Err("loop end must be after its start".into()),
        }
        Ok(())
    }

    /// Names the devices whose helpers hold a change the document has not
    /// recorded yet (ADR 0229). A later update that leaves such a device's
    /// recorded state unchanged keeps its helper and that change. One that
    /// changes it, such as Undo of that device, still loads the recorded bytes.
    pub fn keep_live_plugin_edits(&mut self, devices: Vec<u128>) {
        self.live_plugin_edits = devices;
    }

    /// The live edits `next` leaves in place: each device is still there
    /// with the same plugin and recorded state.
    fn live_edits_kept_by(&self, next: &Project) -> Vec<u128> {
        let recorded = |project: &Project, device: u128| {
            project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .find(|item| item.id.raw() == device)
                .map(|item| (item.plugin.clone(), item.state.clone(), item.offline))
        };
        self.live_plugin_edits
            .iter()
            .copied()
            .filter(|&device| {
                let before = recorded(&self.project, device);
                before.is_some() && before == recorded(next, device)
            })
            .collect()
    }

    pub fn update(&mut self, project: Arc<Project>) {
        if Arc::ptr_eq(&self.project, &project) {
            return;
        }
        let live = self.live_edits_kept_by(&project);
        let diff = self.project.diff(&project);
        if diff.is_empty() || Self::plugin_state_only(&diff) {
            // A hash the helper saved is already sounding. Undo or Redo of a
            // recorded state loads those bytes into the same helper instead;
            // rebuilding here would drop a fader preview and restart every helper.
            let devices: Vec<u128> = diff
                .changes()
                .iter()
                .filter_map(|change| match change {
                    echode_model::Change::Update {
                        id: echode_model::ObjectKey::Id(id),
                        ..
                    } => Some(id.raw()),
                    _ => None,
                })
                .collect();
            let synced = match &mut self.stream {
                Some(stream) if !devices.is_empty() => {
                    stream.sync_plugin_states(&project, &devices, &live)
                }
                _ => Ok(()),
            };
            self.project = project;
            if let Err(error) = synced {
                self.park();
                self.playing = false;
                self.sync_activity();
                self.error = Some(error);
            }
            return;
        }
        let tick = self
            .stream
            .as_ref()
            .map_or(self.tick, |stream| stream.status().tick);
        let changed_tempo = self.project.tempo() != project.tempo();
        self.project = project;
        // Stopped, new media decodes on another thread and the next Play waits
        // for it. Playing, the edit keeps the synchronous order with its locate.
        let running = self.playing && self.cancellation.is_current(self.play_revision);
        let mut replaced = if let Some(stream) = &mut self.stream {
            stream.load_media_in_background(!running);
            stream.replace(&self.project, &live).and_then(|_| {
                if changed_tempo {
                    stream.locate(&self.project, tick)
                } else {
                    Ok(())
                }
            })
        } else {
            Ok(())
        };
        if self.fader_preview.take().is_some()
            && let Some(stream) = &mut self.stream
            && let Err(error) = stream.clear_fader_preview(true)
        {
            replaced = Err(error);
        }
        if let Err(error) = replaced {
            self.park();
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        } else if changed_tempo && let Err(error) = self.resend_loop() {
            self.park();
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        }
    }

    /// The helper hosting a running device. A device view waits for its
    /// replies on another thread, so a slow helper does not hold this one.
    /// Nothing is recorded until the caller journals a captured state.
    ///
    /// The first request of a stopped session opens the output stopped at
    /// the current position. Helpers start on another thread meanwhile.
    pub fn device_control(&mut self, device: echode_model::Id) -> Result<DeviceAccess, String> {
        if let Some(stream) = &self.stream {
            return match stream.device_control(device.raw()) {
                Ok(control) => Ok(DeviceAccess::Ready(control)),
                Err(_) if stream.preparing_graph() => Ok(DeviceAccess::Opening),
                Err(error) => Err(error),
            };
        }
        if self.opening.is_none() {
            if !self
                .project
                .tracks()
                .iter()
                .flat_map(|track| &track.devices)
                .any(|device| device.plugin.is_some() && !device.offline)
            {
                return Err("the project has no running plugin".into());
            }
            self.engine_held = true;
            if !self.refreshed || self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
            {
                self.refresh()?;
            }
            self.error = None;
            self.reconcile()?;
        }
        match (&self.stream, &self.opening) {
            (Some(stream), _) => stream.device_control(device.raw()).map(DeviceAccess::Ready),
            (None, Some(_)) => Ok(DeviceAccess::Opening),
            (None, None) => Err(self.missing_output()),
        }
    }

    /// True while initial output or replacement helpers are being prepared.
    pub fn opening(&self) -> bool {
        self.opening.is_some()
            || self
                .stream
                .as_ref()
                .is_some_and(OutputStream::preparing_graph)
    }

    /// For a request that waited on [`DeviceAccess::Opening`]: the helper
    /// hosting `device`, or why the output did not open. Never opens again.
    pub fn opened_device_control(&self, device: echode_model::Id) -> Result<PluginControl, String> {
        match &self.stream {
            Some(stream) => stream.device_control(device.raw()),
            None => Err(self.missing_output()),
        }
    }

    /// The helper hosting `device`, only when the output is already open.
    /// Never opens the output, so a request with nothing to do stays cheap.
    pub fn running_device_control(&self, device: echode_model::Id) -> Option<PluginControl> {
        self.stream.as_ref()?.device_control(device.raw()).ok()
    }

    fn missing_output(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| "no output device is available".into())
    }

    pub fn meters(&self) -> Vec<Arc<crate::TrackMeter>> {
        self.stream
            .as_ref()
            .map_or_else(Vec::new, OutputStream::meters)
    }

    fn resend_loop(&mut self) -> Result<(), String> {
        let Some((start, end)) = self.loop_ticks else {
            return Ok(());
        };
        if let Some(stream) = &mut self.stream {
            stream.set_loop(&self.project, start, end)?;
        }
        Ok(())
    }

    fn plugin_state_only(diff: &echode_model::ChangeSet) -> bool {
        !diff.is_empty()
            && diff.changes().iter().all(|change| {
                matches!(
                    change,
                    echode_model::Change::Update {
                        property: echode_model::Property::PluginState,
                        ..
                    }
                )
            })
    }

    /// Window notices and plugin-made parameter changes, by device.
    pub fn take_plugin_activity(&self) -> Vec<(u128, PluginActivity)> {
        self.stream
            .as_ref()
            .map(OutputStream::take_plugin_activity)
            .unwrap_or_default()
    }

    /// Plugin bytes the document thread still has to journal.
    pub fn take_plugin_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        self.stream
            .as_mut()
            .map(OutputStream::take_plugin_saves)
            .unwrap_or_default()
    }

    pub fn service(&mut self) {
        if self.refreshed
            && self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
            && let Err(error) = self.refresh().and_then(|_| self.reconcile())
        {
            self.park();
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        }
        self.retiring.retain(|opening| !opening.finished());
        if let Some(result) = self.opening.as_mut().and_then(Opening::poll)
            && let Some(opening) = self.opening.take()
            && let Err(error) = result.and_then(|prepared| self.install(opening, prepared))
        {
            self.park();
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        }
        if let Some(stream) = &mut self.stream
            && let Err(error) = stream.service()
        {
            self.resume_held = false;
            // Recheck format even when a driver changed it before its listener
            // notification arrived. A valid reconfiguration resumes at the beat.
            let old = stream.device.clone();
            let old_workgroup_changed = stream.workgroup_changed();
            match self.refresh().and_then(|_| self.reconcile()) {
                Ok(())
                    if self.stream.as_ref().is_none_or(|stream| {
                        stream.device != old
                            || (old_workgroup_changed && !stream.workgroup_changed())
                    }) => {}
                _ => {
                    self.park();
                    self.playing = false;
                    self.sync_activity();
                    self.error = Some(error);
                }
            }
        }
        if self.resume_held
            && self
                .stream
                .as_ref()
                .is_some_and(|stream| !stream.loading_media())
            && self.playing
            && self.cancellation.is_current(self.play_revision)
            && let Err(error) = self.resume()
        {
            self.cancellation.request(&AudioCommand::Stop);
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        }
    }

    pub fn state(&self) -> AudioState {
        let status = self.stream.as_ref().map(OutputStream::status);
        AudioState {
            playing: self.playing && self.cancellation.is_current(self.play_revision),
            running: self.playing
                && self.cancellation.is_current(self.play_revision)
                && status.is_some_and(|status| status.playing && !status.failed),
            tick: status.map_or(self.tick, |status| status.tick),
            start: self.start,
            paused: self.paused,
            selected_uid: self.selected_uid.clone(),
            active_device: desired(&self.inventory, self.selected_uid.as_deref()).cloned(),
            devices: self.inventory.devices.clone(),
            error: self.error.clone(),
            loop_start: self.loop_ticks.map(|(start, _)| start),
            loop_end: self.loop_ticks.map(|(_, end)| end),
            metronome: self.metronome,
            plugin_missed_blocks: status.map_or(0, |status| status.plugin_missed_blocks),
            graph_unavailable_blocks: status.map_or(0, |status| status.graph_unavailable_blocks),
            pending_plan: status.is_some_and(|status| status.pending_plan),
            audition: self.audition.as_ref().map(crate::Audition::report),
            opening: self.opening.is_some(),
        }
    }

    pub fn position(&self) -> (i64, bool) {
        (
            self.stream
                .as_ref()
                .map_or(self.tick, |stream| stream.status().tick),
            self.playing && self.cancellation.is_current(self.play_revision),
        )
    }

    fn refresh(&mut self) -> Result<(), String> {
        let epoch = self.hardware_epoch.load(Ordering::Acquire);
        let (inventory, notifications) = self.outputs.refresh(&self.hardware_epoch)?;
        self.notifications = notifications;
        self.refreshed = true;
        self.inventory = inventory;
        self.observed_epoch = epoch;
        Ok(())
    }

    fn reconcile(&mut self) -> Result<(), String> {
        let device = desired(&self.inventory, self.selected_uid.as_deref()).cloned();
        if self.stream.as_ref().is_some_and(|stream| {
            Some(&stream.device) != device.as_ref() || stream.workgroup_changed()
        }) {
            self.park();
        }
        if self.opening.as_ref().is_some_and(|opening| {
            opening.device() != device.as_ref() || opening.workgroup_changed()
        }) && let Some(opening) = self.opening.take()
        {
            // Its helpers cannot be stopped mid-start. They exit with the
            // result once the thread is done.
            self.retiring.push(opening);
        }
        if (self.playing || self.engine_held) && self.stream.is_none() && self.opening.is_none() {
            if let Some(device) = device {
                self.opening = Some(self.begin(device)?);
            } else {
                self.error = Some("no output device is available".into());
            }
        }
        if let Some(stream) = &self.stream {
            stream.acknowledge_configuration(self.observed_epoch);
            self.error = None;
        }
        Ok(())
    }

    /// Claims `device` here and prepares its helpers and media on another
    /// thread, from the state at this turn.
    fn begin(&mut self, device: OutputDevice) -> Result<Opening, String> {
        let claim = self.outputs.claim(device)?;
        let transport = crate::stream::TransportSetup {
            loop_ticks: self.loop_ticks,
            metronome: self.metronome,
            clips: Arc::clone(&self.clips),
            package: self.package.clone(),
            cancellation: self.cancellation.clone(),
            revision: self.play_revision,
            fader_preview: self.fader_preview,
            start: self.playing,
        };
        let basis = Basis {
            project: Arc::clone(&self.project),
            tick: self.tick,
            loop_ticks: self.loop_ticks,
            metronome: self.metronome,
            fader_preview: self.fader_preview,
            start: self.playing,
            revision: self.play_revision,
        };
        Opening::begin(Arc::clone(&self.outputs), claim, basis, transport)
    }

    /// Sends what changed while the output was prepared, as if it had been
    /// open throughout, then starts its callback: the first block already
    /// has every change.
    fn install(
        &mut self,
        opening: Opening,
        prepared: crate::stream::Prepared,
    ) -> Result<(), String> {
        let (claim, basis) = opening.finish()?;
        self.stream = Some(OutputStream::assemble(
            claim,
            prepared,
            crate::stream::Start {
                cancellation: self.cancellation.clone(),
                revision: basis.revision,
                package: self.package.clone(),
                hardware_epoch: Arc::clone(&self.hardware_epoch),
                observed_epoch: self.observed_epoch,
                silent: false,
            },
        ));
        let edited = !Arc::ptr_eq(&basis.project, &self.project);
        if edited {
            // An edit while preparing already ended any fader preview. One
            // started after it still applies below.
            let current = std::mem::replace(&mut self.project, Arc::clone(&basis.project));
            let preview = self.fader_preview.take();
            self.update(current);
            self.fader_preview = preview;
            if self.stream.is_none() {
                return Err(self.missing_output());
            }
        }
        let project = Arc::clone(&self.project);
        let stream = self
            .stream
            .as_mut()
            .ok_or("the output closed while it opened")?;
        if self.tick != basis.tick {
            stream.locate(&project, self.tick)?;
        }
        if self.loop_ticks != basis.loop_ticks {
            match self.loop_ticks {
                Some((start, end)) => stream.set_loop(&project, start, end)?,
                None => stream.clear_loop()?,
            }
        }
        if self.metronome != basis.metronome {
            stream.set_metronome(self.metronome)?;
        }
        if self.fader_preview != basis.fader_preview {
            match self.fader_preview {
                Some((track, gain, pan)) => stream.preview_fader(track, gain, pan)?,
                // The edit that ended it holds the value until its plan applies.
                None => stream.clear_fader_preview(edited)?,
            }
        }
        if self.playing && self.cancellation.is_current(self.play_revision) {
            if !(basis.start && basis.revision == self.play_revision) {
                stream.resume(self.play_revision)?;
            }
        } else if basis.start {
            stream.pause()?;
        }
        stream.acknowledge_configuration(self.observed_epoch);
        self.outputs.register(stream)?;
        stream.prepare_in_background();
        self.error = None;
        Ok(())
    }

    fn park(&mut self) {
        // A reopened output starts at install instead.
        self.resume_held = false;
        if let Some(mut stream) = self.stream.take() {
            let status = stream.close();
            self.tick = status.tick;
            if status.failed {
                self.error = Some("the audio driver could not release its callback".into());
            }
        }
    }
}

pub(crate) fn desired<'a>(inventory: &'a Inventory, uid: Option<&str>) -> Option<&'a OutputDevice> {
    uid.and_then(|uid| inventory.devices.iter().find(|device| device.uid == uid))
        .or_else(|| {
            inventory
                .devices
                .iter()
                .find(|device| device.id == inventory.default_device)
        })
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
