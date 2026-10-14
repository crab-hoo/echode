use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_engine::ClipLoader;
use echode_model::Project;
use serde::{Deserialize, Serialize};

use crate::{Inventory, OutputDevice, OutputStream, hardware};

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
    loop_ticks: Option<(i64, i64)>,
    metronome: bool,
    clips: ClipLoader,
    package: Option<PathBuf>,
    error: Option<String>,
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
            loop_ticks: None,
            metronome: false,
            clips: Arc::new(|_project, _rate| Ok(Vec::new())),
            package: None,
            error: None,
        }
    }

    /// Playback reads audio objects from `package`.
    pub fn with_media(project: Arc<Project>, package: &Path) -> Self {
        let mut session = Self::new(project);
        session.clips = crate::clips::media_loader(package.to_path_buf());
        session.package = Some(package.to_path_buf());
        session
    }

    pub fn perform(&mut self, command: AudioCommand) -> Result<(), String> {
        match command {
            AudioCommand::Play => {
                self.error = None;
                self.refresh()?;
                self.playing = true;
                if let Err(error) = self.reconcile() {
                    self.playing = false;
                    self.error = Some(error.clone());
                    return Err(error);
                }
            }
            AudioCommand::Pause => {
                self.park();
                self.playing = false;
            }
            AudioCommand::Stop => {
                // Park keeps the playhead. The next play opens at that tick.
                self.park();
                self.playing = false;
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
        Ok(())
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
        let tick = self
            .stream
            .as_ref()
            .map_or(self.tick, |stream| stream.status().tick);
        let changed_tempo = self.project.tempo() != project.tempo();
        self.project = project;
        let replaced = if let Some(stream) = &mut self.stream {
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
        if let Err(error) = replaced {
            self.park();
            self.playing = false;
            self.error = Some(error);
        } else if changed_tempo && let Err(error) = self.resend_loop() {
            self.park();
            self.playing = false;
            self.error = Some(error);
        }
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
            self.error = Some(error);
        }
        if let Some(stream) = &mut self.stream
            && let Err(error) = stream.service()
        {
            // Recheck format even when a driver changed it before its listener
            // notification arrived. A valid reconfiguration resumes at the beat.
            let old = stream.device.clone();
            match self.refresh().and_then(|_| self.reconcile()) {
                Ok(())
                    if self
                        .stream
                        .as_ref()
                        .is_none_or(|stream| stream.device != old) => {}
                _ => {
                    self.park();
                    self.playing = false;
                    self.error = Some(error);
                }
            }
        }
    }

    pub fn state(&self) -> AudioState {
        let status = self.stream.as_ref().map(OutputStream::status);
        AudioState {
            playing: self.playing,
            running: status.is_some_and(|status| status.playing && !status.failed),
            tick: status.map_or(self.tick, |status| status.tick),
            selected_uid: self.selected_uid.clone(),
            active_device: desired(&self.inventory, self.selected_uid.as_deref()).cloned(),
            devices: self.inventory.devices.clone(),
            error: self.error.clone(),
            loop_start: self.loop_ticks.map(|(start, _)| start),
            loop_end: self.loop_ticks.map(|(_, end)| end),
            metronome: self.metronome,
        }
    }

    pub fn position(&self) -> (i64, bool) {
        (
            self.stream
                .as_ref()
                .map_or(self.tick, |stream| stream.status().tick),
            self.playing,
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
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| Some(&stream.device) != device.as_ref())
        {
            self.park();
        }
        if self.playing && self.stream.is_none() {
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
mod tests {
    use super::*;
    fn device(id: u32, uid: &str) -> OutputDevice {
        OutputDevice {
            id,
            uid: uid.into(),
            name: uid.into(),
            sample_rate: 48000,
            buffer_frames: 128,
            channels: 2,
            formats: Vec::new(),
        }
    }
    #[test]
    fn a_pinned_uid_falls_back_and_returns_with_a_new_hal_id() {
        let mut inventory = Inventory {
            default_device: 1,
            devices: vec![device(1, "speaker"), device(2, "headphone")],
        };
        assert_eq!(desired(&inventory, None).unwrap().id, 1);
        assert_eq!(desired(&inventory, Some("headphone")).unwrap().id, 2);
        inventory.default_device = 2;
        assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 1);
        inventory.devices.remove(0);
        assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 2);
        inventory.devices.push(device(99, "speaker"));
        assert_eq!(desired(&inventory, Some("speaker")).unwrap().id, 99);
        inventory.devices.clear();
        assert!(desired(&inventory, Some("speaker")).is_none());
    }
    #[test]
    fn transport_status_stop_and_seek_do_not_query_hardware_or_edit_the_project() {
        let project = Arc::new(Project::new());
        let mut session = Session::new(project.clone());
        session
            .perform(AudioCommand::Locate { tick: 705600 })
            .unwrap();
        session.perform(AudioCommand::Pause).unwrap();
        assert_eq!(session.state().tick, 705600);
        session.perform(AudioCommand::Stop).unwrap();
        assert_eq!(session.state().tick, 705600);
        assert!(session.perform(AudioCommand::Locate { tick: -1 }).is_err());
        session
            .perform(AudioCommand::Metronome { enabled: true })
            .unwrap();
        session
            .perform(AudioCommand::Loop {
                start: Some(0),
                end: Some(705_600),
            })
            .unwrap();
        assert!(session.state().metronome);
        assert_eq!(session.state().loop_start, Some(0));
        assert_eq!(session.state().loop_end, Some(705_600));
        assert!(
            session
                .perform(AudioCommand::Loop {
                    start: Some(4),
                    end: Some(4),
                })
                .is_err()
        );
        assert_eq!(session.state().loop_end, Some(705_600));
        session
            .perform(AudioCommand::Loop {
                start: None,
                end: None,
            })
            .unwrap();
        assert_eq!(session.state().loop_start, None);
        assert!(session.notifications.is_none());
        assert!(Arc::ptr_eq(&session.project, &project));
    }

    #[test]
    #[ignore = "needs CoreAudio; make spike-audio runs the silent product session"]
    fn a_live_session_refreshes_edits_seeks_and_closes_without_a_window() {
        use std::time::{Duration, Instant};
        let project = Arc::new(Project::new());
        let mut session = Session::new(project.clone());
        session.perform(AudioCommand::Refresh).unwrap();
        let uid = session.state().active_device.expect("output").uid;
        session
            .perform(AudioCommand::Output {
                uid: Some(uid.clone()),
            })
            .unwrap();
        session.perform(AudioCommand::Play).unwrap();
        let pump = |session: &mut Session| {
            let until = Instant::now() + Duration::from_millis(300);
            while Instant::now() < until {
                session.service();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(session.state().error.is_none());
        };
        pump(&mut session);
        assert!(session.state().running && session.state().tick > 0);
        session.update(Arc::new(project.add_track("silent").0));
        pump(&mut session);
        assert!(session.state().running);
        session.perform(AudioCommand::Pause).unwrap();
        let held = session.state().tick;
        pump(&mut session);
        assert_eq!(session.state().tick, held);
        session
            .perform(AudioCommand::Locate { tick: 705600 })
            .unwrap();
        session.perform(AudioCommand::Play).unwrap();
        pump(&mut session);
        assert!(session.state().running && session.state().tick > 705600);
        assert_eq!(session.state().selected_uid, Some(uid));
        let before_stop = session.state().tick;
        session.perform(AudioCommand::Stop).unwrap();
        assert!(!session.state().playing && session.state().tick >= before_stop);
    }
}
