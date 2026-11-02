//! Typed, control-only operations. Each operation owns its validation and wire
//! exchange here; `ControlState` only manages correlation, notices and saved bytes.

use super::{ControlMessage, PARAMETER_REPLY, PluginControl, SessionError, WINDOW_REPLY};
use crate::{GuiReport, ParamChange, ParamInfo, ParamText, ParamValue};
use std::time::Duration;

impl PluginControl {
    /// Opens the plugin's window in its helper, or raises it when it is open.
    /// The window does not take keyboard focus. `title` names a new window.
    pub fn show_gui(&self, title: &str) -> Result<GuiReport, SessionError> {
        let mut state = self.lock();
        let id = state.request(ControlMessage::GuiShow(title.to_string()))?;
        match state.wait_reply(id, WINDOW_REPLY, |message| {
            matches!(
                message,
                ControlMessage::GuiShown(_) | ControlMessage::GuiRejected(_)
            )
        })? {
            ControlMessage::GuiShown(report) => Ok(report),
            ControlMessage::GuiRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not show the window".to_string(),
            )),
        }
    }

    /// Closes the plugin's window. Closing a window that is not open succeeds.
    pub fn close_gui(&self) -> Result<(), SessionError> {
        let mut state = self.lock();
        let id = state.request(ControlMessage::GuiClose)?;
        match state.wait_reply(id, WINDOW_REPLY, |message| {
            matches!(
                message,
                ControlMessage::GuiClosedReply | ControlMessage::GuiRejected(_)
            )
        })? {
            ControlMessage::GuiClosedReply => Ok(()),
            ControlMessage::GuiRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not close the window".to_string(),
            )),
        }
    }

    /// Every parameter the plugin describes, with its value and text.
    pub fn parameters(&self) -> Result<Vec<ParamInfo>, SessionError> {
        let mut state = self.lock();
        let id = state.request(ControlMessage::ParamsRequest)?;
        match state.wait_reply(id, PARAMETER_REPLY, |message| {
            matches!(
                message,
                ControlMessage::Params(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::Params(infos) => Ok(infos),
            ControlMessage::ParamsRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not return parameters".to_string(),
            )),
        }
    }

    /// Applies `changes` in order and returns those parameters as read back.
    pub fn set_parameters(&self, changes: &[ParamChange]) -> Result<Vec<ParamValue>, SessionError> {
        if changes.iter().any(|change| !change.value.is_finite()) {
            return Err(SessionError::Protocol(
                "parameter value is not finite".to_string(),
            ));
        }
        let mut state = self.lock();
        let id = state.request(ControlMessage::SetParams(changes.to_vec()))?;
        match state.wait_reply(id, PARAMETER_REPLY, |message| {
            matches!(
                message,
                ControlMessage::ParamsApplied(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::ParamsApplied(values) => Ok(values),
            ControlMessage::ParamsRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not apply parameters".to_string(),
            )),
        }
    }

    /// Reads typed text for one parameter with the plugin's `text_to_value`.
    /// The plugin's value does not change.
    pub fn parse_parameter(&self, id: u32, text: &str) -> Result<ParamText, SessionError> {
        let mut state = self.lock();
        let request = state.request(ControlMessage::ParseParam {
            id,
            text: text.to_string(),
        })?;
        match state.wait_reply(request, PARAMETER_REPLY, |message| {
            matches!(
                message,
                ControlMessage::ParamParsed(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::ParamParsed(parsed) => Ok(parsed),
            ControlMessage::ParamsRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not read the parameter text".to_string(),
            )),
        }
    }

    /// Saves after every change already sent on this socket has been applied.
    pub fn capture_state(&self) -> Result<Vec<u8>, SessionError> {
        let mut state = self.lock();
        // A background save already in flight answers first on this socket.
        // Collect it before requesting the state after the edits.
        if let Some(id) = state.save_requested.take() {
            state.wait_reply(id, Duration::from_secs(2), |message| {
                matches!(
                    message,
                    ControlMessage::State(_) | ControlMessage::StateRejected(_)
                )
            })?;
        }
        state.save_state()
    }

    /// Loads recorded bytes into the running plugin.
    pub fn restore_state(&self, bytes: &[u8]) -> Result<(), SessionError> {
        let mut state = self.lock();
        let id = state.request(ControlMessage::Restore(bytes.to_vec()))?;
        match state.wait_reply(id, Duration::from_secs(2), |message| {
            matches!(
                message,
                ControlMessage::Restored | ControlMessage::StateRejected(_)
            )
        })? {
            ControlMessage::Restored => {
                state.remember(id, bytes);
                Ok(())
            }
            ControlMessage::StateRejected(reason) => Err(SessionError::Protocol(reason)),
            _ => Err(SessionError::Protocol(
                "helper did not acknowledge restore".to_string(),
            )),
        }
    }
}
