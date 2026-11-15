//! Typed, control-only operations. Each operation owns its validation and wire
//! exchange here; `ControlState` only manages correlation, notices and saved bytes.

use super::{
    ControlMessage, PARAMETER_REPLY, PluginControl, STATE_REPLY, SessionError, WINDOW_REPLY,
};
use crate::error::ProtocolError;
use crate::{AppliedState, GuiReport, ParamChange, ParamInfo, ParamText, ParamValue};
use std::time::Instant;

impl PluginControl {
    /// Reads the current preset list while the helper is deactivated, then
    /// reactivates it. Control-only; interrupts audio, even for a read. No cache
    /// is kept, so a plugin's rescan is visible on the next explicit query.
    pub fn audio_port_configs(&self) -> Result<Vec<crate::AudioPortConfig>, SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = state.request(ControlMessage::AudioConfigsRequest, deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::AudioConfigs(_) | ControlMessage::AudioConfigRejected(_)
            )
        })? {
            ControlMessage::AudioConfigs(configs) => Ok(configs),
            ControlMessage::AudioConfigRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("read audio configurations").into()),
        }
    }

    /// Selects a plugin preset while deactivated, then rereads its ports and
    /// reactivates it. The engine follows port notices with its normal plan
    /// exchange; standalone callers use `PluginHost::follow_ports`. A timeout
    /// has an unknown outcome and must not be automatically retried.
    pub fn select_audio_port_config(&self, config_id: u32) -> Result<(), SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = state.request(ControlMessage::SelectAudioConfig(config_id), deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::AudioConfigSelected | ControlMessage::AudioConfigRejected(_)
            )
        })? {
            ControlMessage::AudioConfigSelected => Ok(()),
            ControlMessage::AudioConfigRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("select audio configuration").into()),
        }
    }
    /// Queries the live tail on the helper's main thread. Control-only: waits
    /// at most the state deadline, so an export cannot commit stale metadata.
    pub fn refresh_tail(&self) -> Result<u32, SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = state.request(ControlMessage::TailRequest, deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::TailChanged(_) | ControlMessage::StateRejected(_)
            )
        })? {
            ControlMessage::TailChanged(samples) => Ok(samples),
            ControlMessage::StateRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("read the tail").into()),
        }
    }

    /// Opens the plugin's window in its helper, or raises it when it is open.
    /// The window does not take keyboard focus. `title` names a new window.
    pub fn show_gui(&self, title: &str) -> Result<GuiReport, SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + WINDOW_REPLY;
        let id = state.request(ControlMessage::GuiShow(title.to_string()), deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::GuiShown(_) | ControlMessage::GuiRejected(_)
            )
        })? {
            ControlMessage::GuiShown(report) => Ok(report),
            ControlMessage::GuiRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("show the window").into()),
        }
    }

    /// Closes the plugin's window. Closing a window that is not open succeeds.
    pub fn close_gui(&self) -> Result<(), SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + WINDOW_REPLY;
        let id = state.request(ControlMessage::GuiClose, deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::GuiClosedReply | ControlMessage::GuiRejected(_)
            )
        })? {
            ControlMessage::GuiClosedReply => Ok(()),
            ControlMessage::GuiRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("close the window").into()),
        }
    }

    /// Every parameter the plugin describes, with its value and text.
    pub fn parameters(&self) -> Result<Vec<ParamInfo>, SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + PARAMETER_REPLY;
        let id = state.request(ControlMessage::ParamsRequest, deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::Params(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::Params(infos) => Ok(infos),
            ControlMessage::ParamsRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("return parameters").into()),
        }
    }

    /// Applies `changes` in order and returns those parameters as read back.
    pub fn set_parameters(&self, changes: &[ParamChange]) -> Result<Vec<ParamValue>, SessionError> {
        if changes.iter().any(|change| !change.value.is_finite()) {
            return Err(SessionError::NonFiniteParameter);
        }
        let mut state = self.lock();
        let deadline = Instant::now() + PARAMETER_REPLY;
        let id = state.request(ControlMessage::SetParams(changes.to_vec()), deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::ParamsApplied(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::ParamsApplied(values) => Ok(values),
            ControlMessage::ParamsRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("apply parameters").into()),
        }
    }

    /// Reads typed text for one parameter with the plugin's `text_to_value`.
    /// The plugin's value does not change.
    pub fn parse_parameter(&self, id: u32, text: &str) -> Result<ParamText, SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + PARAMETER_REPLY;
        let request = state.request(
            ControlMessage::ParseParam {
                id,
                text: text.to_string(),
            },
            deadline,
        )?;
        match state.wait_reply(request, deadline, |message| {
            matches!(
                message,
                ControlMessage::ParamParsed(_) | ControlMessage::ParamsRejected(_)
            )
        })? {
            ControlMessage::ParamParsed(parsed) => Ok(parsed),
            ControlMessage::ParamsRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("read the parameter text").into()),
        }
    }

    /// Saves after every change already sent on this socket has been applied.
    pub fn capture_state(&self) -> Result<Vec<u8>, SessionError> {
        let mut state = self.lock();
        // A background save already in flight answers first on this socket.
        // Collect it before requesting the state after the edits.
        let background = state.save_requested.take();
        let captured = (|| {
            if let Some(id) = background {
                state.wait_reply(id, Instant::now() + STATE_REPLY, |message| {
                    matches!(
                        message,
                        ControlMessage::State(_) | ControlMessage::StateRejected(_)
                    )
                })?;
            }
            state.save_state()
        })();
        if background.is_some()
            && let Err(error) = &captured
        {
            // The foreground read took ownership of an unrecorded background
            // change. Return that change to the poller, not the failed commit.
            let request = state.next_request;
            state.save_failed(Instant::now(), request, error);
        }
        captured
    }

    /// Loads recorded bytes into the running plugin.
    pub fn restore_state(&self, bytes: &[u8]) -> Result<(), SessionError> {
        self.restore_applied(&AppliedState::new(bytes))
    }

    /// Loads `applied` into the running plugin. On success the helper holds
    /// it, together with any name it already carries.
    pub fn restore_applied(&self, applied: &AppliedState) -> Result<(), SessionError> {
        let mut state = self.lock();
        let deadline = Instant::now() + STATE_REPLY;
        let id = state.request(ControlMessage::Restore(applied.bytes().to_vec()), deadline)?;
        match state.wait_reply(id, deadline, |message| {
            matches!(
                message,
                ControlMessage::Restored | ControlMessage::StateRejected(_)
            )
        })? {
            ControlMessage::Restored => {
                state.remember(id, applied.clone());
                Ok(())
            }
            ControlMessage::StateRejected(refusal) => Err(refusal.into()),
            _ => Err(ProtocolError::UnexpectedReply("acknowledge restore").into()),
        }
    }
}
