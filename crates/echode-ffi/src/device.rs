//! Parameter access for a running device. Values change in its helper; the
//! project changes only when a captured state is recorded as one Undo step.

use std::collections::BTreeSet;

use echode_actions::Command;
use echode_audio_io::{ParamChange, ParamInfo, ParamValue, Session};
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

/// Runs one device command on the document worker and returns its JSON report.
///
/// `editing` holds devices whose values changed since their last commit. The
/// worker defers background saves for them: a plugin that marks itself dirty
/// after a host change must not turn the edit into a non-undoable capture.
pub(crate) fn device_report(
    host: &DocumentHost,
    audio: &mut Session,
    editing: &mut BTreeSet<u128>,
    bytes: &[u8],
) -> Result<Vec<u8>, String> {
    let command: DeviceCommand =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let report = match command {
        DeviceCommand::Parameters { device } => {
            hosted(host, device)?;
            let infos = audio.device_parameters(device)?;
            serde_json::json!({ "parameters": infos.iter().map(parameter).collect::<Vec<_>>() })
        }
        DeviceCommand::SetParameters { device, changes } => {
            hosted(host, device)?;
            if changes.is_empty() || changes.iter().any(|change| !change.value.is_finite()) {
                return Err("parameter changes are empty or not finite".into());
            }
            let changes: Vec<_> = changes
                .iter()
                .map(|change| ParamChange {
                    id: change.id,
                    value: change.value,
                })
                .collect();
            editing.insert(device.raw());
            let values = audio.set_device_parameters(device, &changes)?;
            serde_json::json!({ "values": values.iter().map(value).collect::<Vec<_>>() })
        }
        DeviceCommand::Commit { device } => {
            hosted(host, device)?;
            let bytes = audio.capture_device_state(device)?;
            let changed = record_state(host, device, &bytes)?;
            editing.remove(&device.raw());
            audio.update(host.snapshot());
            serde_json::json!({ "changed": changed })
        }
    };
    serde_json::to_vec(&report).map_err(|error| error.to_string())
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
        assert!(device_report(&host, &mut audio, &mut editing, unknown).is_err());
        let mixer_request = format!(r#"{{"type":"device.parameters","device":"{mixer}"}}"#);
        let error =
            device_report(&host, &mut audio, &mut editing, mixer_request.as_bytes()).unwrap_err();
        assert!(error.contains("mixer"), "{error}");
        let empty = format!(r#"{{"type":"device.parameter.set","device":"{mixer}","changes":[]}}"#);
        assert!(device_report(&host, &mut audio, &mut editing, empty.as_bytes()).is_err());
        assert!(editing.is_empty());
        drop(host);
        let _ = std::fs::remove_dir_all(dir);
    }
}
