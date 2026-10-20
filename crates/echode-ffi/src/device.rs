//! Parameter access for a running device. Values change in its helper; the
//! project changes only when a captured state is recorded as one Undo step.
//! The plugin's own window opens and closes the same way (ADR 0132).
//!
//! The document worker checks a command and resolves the helper. The round
//! trip then runs on the device lane, so a plugin that is slow to answer
//! does not delay edits, transport or other devices. Captured bytes return to
//! the worker, which records them in order with every other edit.

use std::collections::BTreeSet;

use echode_actions::Command;
use echode_audio_io::{
    GuiReport, ParamChange, ParamInfo, ParamValue, PluginActivity, PluginControl, Session,
};
use echode_history::DocumentHost;
use echode_model::Id;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum DeviceCommand {
    /// Descriptions and values, read from the helper.
    #[serde(rename = "device.parameters")]
    Parameters { device: Id },
    /// Values applied in order. The project is unchanged.
    #[serde(rename = "device.parameter.set")]
    SetParameters { device: Id, changes: Vec<Change> },
    /// Records the helper's state, once, if it differs from the project.
    #[serde(rename = "device.parameter.commit")]
    Commit { device: Id },
    /// Opens the plugin's own window in its helper, or raises it. The window
    /// does not take keyboard focus. `title` names a new window.
    #[serde(rename = "device.gui.show")]
    ShowGui { device: Id, title: String },
    /// Closes the plugin's own window. A device with no running helper has none.
    #[serde(rename = "device.gui.close")]
    CloseGui { device: Id },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    id: u32,
    value: f64,
}

#[derive(Serialize)]
struct Parameter<'a> {
    id: u32,
    flags: u32,
    name: &'a str,
    module: &'a str,
    min: f64,
    max: f64,
    default: f64,
    value: f64,
    text: &'a str,
}

#[derive(Serialize)]
struct Value<'a> {
    id: u32,
    value: f64,
    text: &'a str,
}

/// What the lane asks the helper.
pub(crate) enum Request {
    Parameters,
    Set(Vec<ParamChange>),
    /// The helper's bytes, which the worker records with [`commit`].
    Capture,
    ShowGui(String),
    CloseGui,
}

/// A checked command and the helper that answers it. Only closing a window
/// can have no helper: then there is no window to close.
pub(crate) struct Dispatch {
    pub(crate) device: Id,
    pub(crate) control: Option<PluginControl>,
    pub(crate) request: Request,
}

/// Checks one device command on the document worker and resolves its helper.
/// The first device view of a stopped session opens the output here.
///
/// `editing` holds devices whose values changed since their last commit. The
/// worker defers background saves for them: a plugin that marks itself dirty
/// after a host change must not turn the edit into a non-undoable capture.
pub(crate) fn dispatch(
    host: &DocumentHost,
    audio: &mut Session,
    editing: &mut BTreeSet<u128>,
    bytes: &[u8],
) -> Result<Dispatch, String> {
    let command: DeviceCommand =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let (device, request) = match command {
        DeviceCommand::Parameters { device } => (device, Request::Parameters),
        DeviceCommand::SetParameters { device, changes } => {
            hosted(host, device)?;
            if changes.is_empty() || changes.iter().any(|change| !change.value.is_finite()) {
                return Err("parameter changes are empty or not finite".into());
            }
            let changes = changes
                .iter()
                .map(|change| ParamChange {
                    id: change.id,
                    value: change.value,
                })
                .collect();
            (device, Request::Set(changes))
        }
        DeviceCommand::Commit { device } => (device, Request::Capture),
        DeviceCommand::ShowGui { device, title } => (device, Request::ShowGui(title)),
        DeviceCommand::CloseGui { device } => {
            // Closing never opens the output or starts a helper.
            return Ok(Dispatch {
                device,
                control: audio.running_device_control(device),
                request: Request::CloseGui,
            });
        }
    };
    hosted(host, device)?;
    let control = Some(audio.device_control(device)?);
    if matches!(request, Request::Set(_)) {
        editing.insert(device.raw());
    }
    Ok(Dispatch {
        device,
        control,
        request,
    })
}

/// Runs on the device lane. Returns the JSON report, or captured bytes.
pub(crate) fn round_trip(
    control: Option<&PluginControl>,
    request: &Request,
) -> Result<Vec<u8>, String> {
    let Some(control) = control else {
        return serde_json::to_vec(&serde_json::json!({ "closed": true }))
            .map_err(|error| error.to_string());
    };
    let report = match request {
        Request::Parameters => {
            let infos = control.parameters().map_err(|error| error.to_string())?;
            serde_json::json!({ "parameters": infos.iter().map(parameter).collect::<Vec<_>>() })
        }
        Request::Set(changes) => {
            let values = control
                .set_parameters(changes)
                .map_err(|error| error.to_string())?;
            serde_json::json!({ "values": values.iter().map(value).collect::<Vec<_>>() })
        }
        Request::Capture => return control.capture_state().map_err(|error| error.to_string()),
        Request::ShowGui(title) => {
            let GuiReport {
                created,
                visible,
                key,
            } = control.show_gui(title).map_err(|error| error.to_string())?;
            serde_json::json!({ "window": { "created": created, "visible": visible, "key": key } })
        }
        Request::CloseGui => {
            control.close_gui().map_err(|error| error.to_string())?;
            serde_json::json!({ "closed": true })
        }
    };
    serde_json::to_vec(&report).map_err(|error| error.to_string())
}

/// Records bytes the lane captured and ends the device's parameter edit.
pub(crate) fn commit(
    host: &DocumentHost,
    audio: &mut Session,
    editing: &mut BTreeSet<u128>,
    device: Id,
    bytes: &[u8],
) -> Result<Vec<u8>, String> {
    let changed = record_state(host, device, bytes)?;
    editing.remove(&device.raw());
    audio.update(host.snapshot());
    serde_json::to_vec(&serde_json::json!({ "changed": changed }))
        .map_err(|error| error.to_string())
}

/// Writes the bytes and names them on `device` as an ordinary edit. Bytes
/// equal to the recorded state leave history unchanged and return `false`.
pub(crate) fn record_state(host: &DocumentHost, device: Id, bytes: &[u8]) -> Result<bool, String> {
    let hash =
        echode_history::write_object(host.directory(), bytes).map_err(|error| error.to_string())?;
    let project = host.snapshot();
    let current = project
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .find(|item| item.id == device)
        .ok_or("the device is no longer in the project")?;
    if current.state.as_deref() == Some(hash.as_str()) {
        return Ok(false);
    }
    host.perform(Command::SetPluginState {
        device,
        state: Some(hash),
    })
    .map_err(|error| error.to_string())?;
    Ok(true)
}

/// True when a plugin's own change will be recorded by a later commit, so a
/// background save of the same change must wait: a gesture, or, while the
/// plugin's window is open, a value set without one, a dirty state or lost
/// events. With the window closed those stay background saves, so a plugin
/// that changes itself after loading adds no Undo step (ADR 0132).
pub(crate) fn defers_saves(activity: &PluginActivity) -> bool {
    use echode_audio_io::ParamEventKind;
    let relay = &activity.relay;
    relay
        .events
        .iter()
        .any(|event| event.kind != ParamEventKind::Value)
        || (activity.window_open && (!relay.events.is_empty() || relay.lost))
        || activity.dirty
}

/// A CLAP device in the current project that is not offline.
fn hosted(host: &DocumentHost, device: Id) -> Result<(), String> {
    let project = host.snapshot();
    let found = project
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .find(|item| item.id == device)
        .ok_or("the device is no longer in the project")?;
    if found.plugin.is_none() {
        return Err("the mixer has no plugin parameters".into());
    }
    if found.offline {
        return Err("the device is offline".into());
    }
    Ok(())
}

fn parameter(info: &ParamInfo) -> Parameter<'_> {
    Parameter {
        id: info.id,
        flags: info.flags,
        name: &info.name,
        module: &info.module,
        min: info.min,
        max: info.max,
        default: info.default,
        value: info.value,
        text: &info.text,
    }
}

fn value(value: &ParamValue) -> Value<'_> {
    Value {
        id: value.id,
        value: value.value,
        text: &value.text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_the_same_bytes_twice_adds_one_undo_step() {
        let dir = std::env::temp_dir().join(format!("echode-ffi-device-{}", Id::generate()));
        let (project, track) = echode_model::Project::new().add_track("fx");
        let device = Id::from_raw(0xD0);
        let project = project
            .add_clap_device(track, device, "org.example.fx")
            .unwrap();
        echode_history::write_package(&dir, &project).unwrap();
        let host = DocumentHost::open(&dir).unwrap();
        assert!(record_state(&host, device, b"edited").unwrap());
        assert!(!record_state(&host, device, b"edited").unwrap());
        let hash = echode_history::object_hash(b"edited");
        let state = |host: &DocumentHost| host.snapshot().tracks()[0].devices[0].state.clone();
        assert_eq!(state(&host), Some(hash.clone()));
        host.undo().unwrap();
        assert_eq!(state(&host), None);
        host.redo().unwrap();
        assert_eq!(state(&host), Some(hash));
        assert!(record_state(&host, Id::from_raw(0xD1), b"gone").is_err());
        drop(host);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn device_commands_refuse_unknown_fields_and_the_mixer() {
        let dir = std::env::temp_dir().join(format!("echode-ffi-device-{}", Id::generate()));
        let (project, _) = echode_model::Project::new().add_track("fx");
        let mixer = project.tracks()[0].devices[0].id;
        echode_history::write_package(&dir, &project).unwrap();
        let host = DocumentHost::open(&dir).unwrap();
        let mut audio = Session::new(host.snapshot());
        let mut editing = BTreeSet::new();
        let unknown = br#"{"type":"device.parameters","device":"00000000-0000-0000-0000-0000000000d0","extra":1}"#;
        assert!(dispatch(&host, &mut audio, &mut editing, unknown).is_err());
        let mixer_request = format!(r#"{{"type":"device.parameters","device":"{mixer}"}}"#);
        let Err(error) = dispatch(&host, &mut audio, &mut editing, mixer_request.as_bytes()) else {
            panic!("the mixer has no helper");
        };
        assert!(error.contains("mixer"), "{error}");
        let empty = format!(r#"{{"type":"device.parameter.set","device":"{mixer}","changes":[]}}"#);
        assert!(dispatch(&host, &mut audio, &mut editing, empty.as_bytes()).is_err());
        let window = format!(r#"{{"type":"device.gui.show","device":"{mixer}","title":"Mix"}}"#);
        let Err(error) = dispatch(&host, &mut audio, &mut editing, window.as_bytes()) else {
            panic!("the mixer has no plugin window");
        };
        assert!(error.contains("mixer"), "{error}");
        assert!(editing.is_empty());
        drop(host);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Closing a window never opens the output or starts a helper to do it:
    /// this unresolvable plugin would fail if the close tried.
    #[test]
    fn closing_a_window_without_a_helper_does_nothing() {
        let dir = std::env::temp_dir().join(format!("echode-ffi-device-{}", Id::generate()));
        let (project, track) = echode_model::Project::new().add_track("fx");
        let device = Id::from_raw(0xD2);
        let project = project
            .add_clap_device(track, device, "org.example.fx")
            .unwrap();
        echode_history::write_package(&dir, &project).unwrap();
        let host = DocumentHost::open(&dir).unwrap();
        let mut audio = Session::new(host.snapshot());
        let mut editing = BTreeSet::new();
        let close = format!(r#"{{"type":"device.gui.close","device":"{device}"}}"#);
        let dispatched = dispatch(&host, &mut audio, &mut editing, close.as_bytes()).unwrap();
        assert!(dispatched.control.is_none());
        let report = round_trip(dispatched.control.as_ref(), &dispatched.request).unwrap();
        assert_eq!(report, br#"{"closed":true}"#);
        drop(host);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_changes_a_commit_records_defer_background_saves() {
        use echode_audio_io::{ParamEvent, ParamEventKind};
        let mut activity = PluginActivity::default();
        activity.relay.rescan = true;
        activity.closed = true;
        assert!(
            !defers_saves(&activity),
            "a rescan or a closed window records nothing"
        );
        let value = ParamEvent {
            id: 1,
            kind: ParamEventKind::Value,
        };
        let mut closed_window = PluginActivity::default();
        closed_window.relay.events.push(value);
        closed_window.relay.lost = true;
        assert!(
            !defers_saves(&closed_window),
            "without a window, a plugin's own value stays a background save"
        );
        for change in [
            |activity: &mut PluginActivity| {
                activity.relay.events.push(ParamEvent {
                    id: 1,
                    kind: ParamEventKind::Begin,
                })
            },
            |activity: &mut PluginActivity| {
                activity.window_open = true;
                activity.relay.events.push(ParamEvent {
                    id: 1,
                    kind: ParamEventKind::Value,
                });
            },
            |activity: &mut PluginActivity| {
                activity.window_open = true;
                activity.relay.lost = true;
            },
            |activity: &mut PluginActivity| activity.dirty = true,
        ] {
            let mut edited = PluginActivity::default();
            change(&mut edited);
            assert!(defers_saves(&edited));
        }
    }
}
