//! Parameter access for a running device. Values change in its helper; the
//! project changes only when a captured state is recorded as one Undo step.
//! The plugin's own window opens and closes the same way (ADR 0132).
//!
//! The document worker checks a command and resolves the helper. The round
//! trip then runs on the device lane, so a plugin that is slow to answer
//! does not delay edits, transport or other devices. Captured bytes return to
//! the worker, which records them in order with every other edit.
//!
//! The first request of a stopped session opens the output, whose helpers
//! start on another thread. That request and every later one wait on the
//! worker, in order, without holding its other turns (ADR 0136).

use std::collections::BTreeSet;

use echode_actions::Command;
use echode_audio_io::{
    DeviceAccess, GuiReport, ParamChange, ParamInfo, ParamText, ParamValue, PluginActivity,
    PluginControl, Session,
};
use echode_history::DocumentHost;
use echode_model::Id;
use serde::{Deserialize, Serialize};

use crate::error::encode;
use crate::{DocumentError, Rejected};

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
    /// Reads typed text with the plugin's `text_to_value`. Nothing changes.
    #[serde(rename = "device.parameter.parse")]
    Parse { device: Id, id: u32, text: String },
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
    Parse(u32, String),
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

/// A checked command whose helper is not known yet: the output is opening,
/// or an earlier request is still waiting for it.
pub(crate) struct Waiting {
    device: Id,
    request: Request,
}

pub(crate) enum Resolution {
    Ready(Dispatch),
    Waiting(Waiting),
}

/// Checks one device command on the document worker and resolves its helper.
/// The first device view of a stopped session starts opening the output here.
/// `behind` is true while earlier requests wait for it; this one then waits
/// too, so the lane receives requests in their submission order.
///
/// `editing` holds devices whose values changed since their last commit. The
/// worker defers background saves for them: a plugin that marks itself dirty
/// after a host change must not turn the edit into a non-undoable capture.
pub(crate) fn dispatch(
    host: &DocumentHost,
    audio: &mut Session,
    editing: &mut BTreeSet<u128>,
    bytes: &[u8],
    behind: bool,
) -> Result<Resolution, DocumentError> {
    let command: DeviceCommand = serde_json::from_slice(bytes).map_err(DocumentError::Malformed)?;
    let (device, request) = match command {
        DeviceCommand::Parameters { device } => (device, Request::Parameters),
        DeviceCommand::SetParameters { device, changes } => {
            hosted(host, device)?;
            if changes.is_empty() || changes.iter().any(|change| !change.value.is_finite()) {
                return Err(Rejected::ParameterChanges.into());
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
        DeviceCommand::Parse { device, id, text } => (device, Request::Parse(id, text)),
        DeviceCommand::ShowGui { device, title } => (device, Request::ShowGui(title)),
        DeviceCommand::CloseGui { device } => (device, Request::CloseGui),
    };
    let waiting = Waiting { device, request };
    if !matches!(waiting.request, Request::CloseGui) {
        hosted(host, device)?;
    }
    if behind {
        return Ok(Resolution::Waiting(waiting));
    }
    if matches!(waiting.request, Request::CloseGui) {
        // Closing never opens the output or starts a helper.
        let control = audio.running_device_control(device);
        return Ok(Resolution::Ready(waiting.ready(control, editing)));
    }
    Ok(match audio.device_control(device)? {
        DeviceAccess::Ready(control) => Resolution::Ready(waiting.ready(Some(control), editing)),
        DeviceAccess::Opening => Resolution::Waiting(waiting),
    })
}

impl Waiting {
    /// Resolves the helper once the output has opened, or reports why it did
    /// not. Never starts another opening. The device may have been removed
    /// or taken offline meanwhile.
    pub(crate) fn resume(
        self,
        host: &DocumentHost,
        audio: &Session,
        editing: &mut BTreeSet<u128>,
    ) -> Result<Dispatch, DocumentError> {
        let control = if matches!(self.request, Request::CloseGui) {
            audio.running_device_control(self.device)
        } else {
            hosted(host, self.device)?;
            Some(audio.opened_device_control(self.device)?)
        };
        Ok(self.ready(control, editing))
    }

    fn ready(self, control: Option<PluginControl>, editing: &mut BTreeSet<u128>) -> Dispatch {
        if matches!(self.request, Request::Set(_)) {
            editing.insert(self.device.raw());
        }
        Dispatch {
            device: self.device,
            control,
            request: self.request,
        }
    }
}

/// Runs on the device lane. Returns the JSON report, or captured bytes. A
/// helper's failure stays [`DocumentError::Helper`], so a timeout is told
/// apart from a refusal.
pub(crate) fn round_trip(
    control: Option<&PluginControl>,
    request: &Request,
) -> Result<Vec<u8>, DocumentError> {
    let Some(control) = control else {
        return encode(&serde_json::json!({ "closed": true }));
    };
    let report = match request {
        Request::Parameters => {
            let infos = control.parameters()?;
            serde_json::json!({
                "parameters": infos.iter().map(parameter).collect::<Vec<_>>(),
                "latency_samples": control.latency_samples(),
            })
        }
        Request::Set(changes) => {
            let values = control.set_parameters(changes)?;
            serde_json::json!({ "values": values.iter().map(value).collect::<Vec<_>>() })
        }
        Request::Capture => return Ok(control.capture_state()?),
        Request::Parse(id, text) => {
            // `converts` is false when the plugin has no `text_to_value`;
            // the host then reads a plain number in the plugin's units.
            let (value, converts) = match control.parse_parameter(*id, text)? {
                ParamText::Value(value) => (Some(value), true),
                ParamText::Refused => (None, true),
                ParamText::Unsupported => (None, false),
            };
            serde_json::json!({ "parsed": { "id": id, "value": value, "converts": converts } })
        }
        Request::ShowGui(title) => {
            let GuiReport {
                created,
                visible,
                key,
            } = control.show_gui(title)?;
            serde_json::json!({ "window": { "created": created, "visible": visible, "key": key } })
        }
        Request::CloseGui => {
            control.close_gui()?;
            serde_json::json!({ "closed": true })
        }
    };
    encode(&report)
}

/// Ends the device's parameter edit, then records the lane's captured bytes.
/// Capture and recording failures still release background saves: the app has
/// finished the gesture and will not send another commit for it.
pub(crate) fn commit(
    host: &DocumentHost,
    audio: &mut Session,
    editing: &mut BTreeSet<u128>,
    device: Id,
    captured: Result<Vec<u8>, DocumentError>,
) -> Result<Vec<u8>, DocumentError> {
    editing.remove(&device.raw());
    let changed = record_state(host, device, &captured?)?;
    audio.update(host.snapshot());
    encode(&serde_json::json!({ "changed": changed }))
}

/// Writes the bytes and names them on `device` as an ordinary edit. Bytes
/// equal to the recorded state leave history unchanged and return `false`.
pub(crate) fn record_state(
    host: &DocumentHost,
    device: Id,
    bytes: &[u8],
) -> Result<bool, DocumentError> {
    let hash = echode_history::write_object(host.directory(), bytes)?;
    let project = host.snapshot();
    let current = project
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .find(|item| item.id == device)
        .ok_or(Rejected::DeviceGone)?;
    if current.state.as_deref() == Some(hash.as_str()) {
        return Ok(false);
    }
    host.perform(Command::SetPluginState {
        device,
        state: Some(hash),
    })?;
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
fn hosted(host: &DocumentHost, device: Id) -> Result<(), Rejected> {
    let project = host.snapshot();
    let found = project
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .find(|item| item.id == device)
        .ok_or(Rejected::DeviceGone)?;
    if found.plugin.is_none() {
        return Err(Rejected::Mixer);
    }
    if found.offline {
        return Err(Rejected::DeviceOffline);
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
#[path = "device_tests.rs"]
mod tests;
