use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_engine::ClipLoader;
use echode_model::Project;
use serde::{Deserialize, Serialize};

use crate::{
    Inventory, OutputDevice, OutputStream, ParamChange, ParamInfo, ParamValue,
    TransportCancellation, hardware,
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
    #[serde(rename = "audio.output")]
    Output { uid: Option<String> },
    #[serde(rename = "audio.refresh")]
    Refresh,
    #[serde(rename = "audio.status")]
    Status,
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
}

/// One foreground project. No hardware is queried until the app asks for the
/// output list or starts playback; package editing stays fully headless.
pub struct Session {
    project: Arc<Project>,
    stream: Option<OutputStream>,
    notifications: Option<hardware::Notifications>,
    hardware_epoch: Arc<AtomicU64>,
    observed_epoch: u64,
    inventory: Inventory,
    selected_uid: Option<String>,
    tick: i64,
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
    /// A device view asked for running plugins. The output stays open while
    /// stopped, as it does after playback.
    engine_held: bool,
}

impl Session {
    pub fn new(project: Arc<Project>) -> Self {
        Self {
            project,
            stream: None,
            notifications: None,
            hardware_epoch: Arc::new(AtomicU64::new(0)),
            observed_epoch: 0,
            inventory: Inventory::default(),
            selected_uid: None,
            tick: 0,
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
        }
    }

    /// Playback reads audio objects from `package`.
    pub fn with_media(project: Arc<Project>, package: &Path) -> Self {
        let mut session = Self::new(project);
        session.clips = crate::clips::media_loader(package.to_path_buf());
        session.package = Some(package.to_path_buf());
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
                let already_playing =
                    self.playing && self.play_revision == revision && self.stream.is_some();
                self.error = None;
                if self.notifications.is_none()
                    || self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
                {
                    self.refresh()?;
                }
                self.play_revision = revision;
                self.playing = true;
                if let Err(error) = self.reconcile() {
                    self.playing = false;
                    self.sync_activity();
                    self.error = Some(error.clone());
                    return Err(error);
                }
                self.playing = self.cancellation.is_current(revision);
                if self.playing
                    && !already_playing
                    && let Some(stream) = &mut self.stream
                    && let Err(error) = stream.resume(revision)
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
                // silenced the callback without waiting for this queue.
                self.playing = false;
                if let Some(stream) = &mut self.stream {
                    self.tick = stream.status().tick;
                    stream.pause()?;
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
            AudioCommand::Output { uid } => {
                self.refresh()?;
                self.selected_uid = uid;
                self.error = None;
                self.reconcile()?;
            }
            AudioCommand::Refresh => {
                self.refresh()?;
                self.reconcile()?;
            }
            AudioCommand::Status => {}
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

    pub fn update(&mut self, project: Arc<Project>) {
        if Arc::ptr_eq(&self.project, &project) {
            return;
        }
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
                    stream.sync_plugin_states(&project, &devices)
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
        let mut replaced = if let Some(stream) = &mut self.stream {
            stream.replace(&self.project).and_then(|_| {
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

    /// Every parameter of a running device, read from its helper.
    pub fn device_parameters(
        &mut self,
        device: echode_model::Id,
    ) -> Result<Vec<ParamInfo>, String> {
        self.open_engine()?.device_parameters(device.raw())
    }

    /// Changes values in the running helper. Nothing is recorded until
    /// [`Self::capture_device_state`] is journaled by the caller.
    pub fn set_device_parameters(
        &mut self,
        device: echode_model::Id,
        changes: &[ParamChange],
    ) -> Result<Vec<ParamValue>, String> {
        self.open_engine()?
            .set_device_parameters(device.raw(), changes)
    }

    /// Bytes including every applied change, for the caller to record.
    pub fn capture_device_state(&mut self, device: echode_model::Id) -> Result<Vec<u8>, String> {
        self.open_engine()?.capture_device_state(device.raw())
    }

    /// Device views need running helpers. Opens the output stopped at the
    /// current position when playback has not opened it yet.
    fn open_engine(&mut self) -> Result<&mut OutputStream, String> {
        if self.stream.is_none() {
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
            if self.notifications.is_none()
                || self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
            {
                self.refresh()?;
            }
            self.reconcile()?;
        }
        let missing = self
            .error
            .clone()
            .unwrap_or_else(|| "no output device is available".into());
        self.stream.as_mut().ok_or(missing)
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

    /// Plugin bytes the document thread still has to journal.
    pub fn take_plugin_saves(&mut self) -> Vec<(u128, Vec<u8>)> {
        self.stream
            .as_mut()
            .map(OutputStream::take_plugin_saves)
            .unwrap_or_default()
    }

    pub fn service(&mut self) {
        if self.notifications.is_some()
            && self.hardware_epoch.load(Ordering::Acquire) != self.observed_epoch
            && let Err(error) = self.refresh().and_then(|_| self.reconcile())
        {
            self.park();
            self.playing = false;
            self.sync_activity();
            self.error = Some(error);
        }
        if let Some(stream) = &mut self.stream
            && let Err(error) = stream.service()
        {
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
    }

    pub fn state(&self) -> AudioState {
        let status = self.stream.as_ref().map(OutputStream::status);
        AudioState {
            playing: self.playing && self.cancellation.is_current(self.play_revision),
            running: self.playing
                && self.cancellation.is_current(self.play_revision)
                && status.is_some_and(|status| status.playing && !status.failed),
            tick: status.map_or(self.tick, |status| status.tick),
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
        let inventory = hardware::inventory()?;
        let notifications =
            hardware::Notifications::new(&inventory, Arc::clone(&self.hardware_epoch))?;
        // Close the enumeration/registration gap. Subsequent changes are
        // covered by the new system/device listeners.
        if hardware::inventory()? != inventory {
            notifications.invalidate();
        }
        self.notifications = Some(notifications);
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
        if (self.playing || self.engine_held) && self.stream.is_none() {
            if let Some(device) = device {
                self.stream = Some(OutputStream::open_guarded(
                    device,
                    &self.project,
                    self.tick,
                    crate::stream::TransportSetup {
                        loop_ticks: self.loop_ticks,
                        metronome: self.metronome,
                        clips: Arc::clone(&self.clips),
                        package: self.package.clone(),
                        cancellation: self.cancellation.clone(),
                        revision: self.play_revision,
                        fader_preview: self.fader_preview,
                        start: self.playing,
                    },
                    Arc::clone(&self.hardware_epoch),
                    self.observed_epoch,
                )?);
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

    fn park(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let status = stream.close();
            self.tick = status.tick;
            if status.failed {
                self.error = Some("the audio driver could not release its callback".into());
            }
        }
    }
}

fn desired<'a>(inventory: &'a Inventory, uid: Option<&str>) -> Option<&'a OutputDevice> {
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
