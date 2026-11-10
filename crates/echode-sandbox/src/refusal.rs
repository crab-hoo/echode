//! The helper's answer to a request it did not complete (ADR 0293).
//!
//! A refusal crosses the control socket as one kind byte and that kind's
//! payload. Busy, a timeout and a stop are kinds of their own, so the host can
//! tell a request that never started from one whose outcome is unknown
//! (ADR 0199). What the plugin lacks or refused is a kind too (ADR 0295).
//! Launch causes keep a typed kind and native metadata alongside the loader's
//! diagnostic (ADR 0308). Internal reply mismatches and invalid parameter ids are typed too.

use crate::{LaunchFailure, MainReplyKind, RefusalDecodeError, RefusalField};
use std::io::{self, ErrorKind};

/// What the helper was waiting for when it gave up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// A call on the plugin's main thread, by the helper's own call number.
    MainCall(u64),
    /// Parameter changes published to the plugin's processing thread.
    ParameterChanges,
}

/// Why the helper answered without the request's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The plugin's main thread still runs an earlier call. This request did
    /// not start and changed nothing, so a later one can succeed.
    Busy,
    /// The helper has no plugin loaded.
    NoPlugin,
    /// The helper stopped waiting before the plugin finished. The plugin may
    /// still complete the request: the outcome is unknown, and the host must
    /// not resend it as if nothing happened (ADR 0199).
    TimedOut(Pending),
    /// The helper began to stop before the plugin finished. The outcome is
    /// unknown, as for [`Self::TimedOut`].
    Stopped(Pending),
    /// The plugin lacks what the request needs, or answered `false`. The
    /// request finished: nothing is pending.
    Plugin(PluginFailure),
    /// Loading or activating the plugin failed. The cause is separate from its
    /// original diagnostic, which is only for display.
    Launch(LaunchFailure),
    /// The reserved CLAP id cannot name a parameter. No batch was published.
    InvalidParameterId(u32),
    /// The helper's own main-thread operation returned the wrong payload.
    UnexpectedMainReply {
        expected: MainReplyKind,
        actual: MainReplyKind,
    },
}

impl From<PluginFailure> for Refusal {
    fn from(failure: PluginFailure) -> Self {
        Self::Plugin(failure)
    }
}

/// One of the plugin's audio ports, as `clap.audio-ports` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPort {
    pub input: bool,
    pub index: u32,
}

impl std::fmt::Display for AudioPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let side = if self.input { "input" } else { "output" };
        write!(f, "{side} port {}", self.index)
    }
}

/// Why the helper could not do what was asked with the plugin (ADR 0295).
///
/// The plugin does not declare a CLAP feature (`No…`), answered `false`
/// (`…Refused`), or described itself in a way the helper cannot use. The two
/// `Panel…` kinds are the helper's own window for the plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFailure {
    /// `clap.state` is missing.
    NoState,
    NoStateSave,
    NoStateLoad,
    SaveRefused,
    LoadRefused,
    /// Echode's event-capacity extension answered `false`.
    CapacityRefused,
    /// `clap.params` has no `flush`, so values cannot be set.
    NoParameterFlush,
    /// `clap.gui` is missing.
    NoWindow,
    /// The plugin has neither an embedded nor a floating Cocoa window.
    NoCocoaWindow,
    NoWindowCreate,
    WindowRefused,
    /// The embedded window has no `set_parent`.
    NoEmbedding,
    /// `set_parent` answered `false`.
    ParentRefused,
    /// The panel was asked for off the main thread.
    PanelNeedsMainThread,
    PanelHasNoView,
    /// `clap.audio-ports` is missing.
    NoAudioPorts,
    NoPortCount,
    NoPortInfo,
    PortUndescribed(AudioPort),
    PortWithoutChannels(AudioPort),
    /// The plugin declares no audio port on either side.
    NoPorts,
    NoNotePortCount,
    NoNotePortInfo,
    NotePortUndescribed,
    /// A MIDI note port's index does not fit in the event's 16-bit port.
    NotePortIndex,
}

impl std::fmt::Display for PluginFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::NoState => "plugin does not declare state",
            Self::NoStateSave => "plugin state has no save",
            Self::NoStateLoad => "plugin state has no load",
            Self::SaveRefused => "plugin refused to save state",
            Self::LoadRefused => "plugin refused to load state",
            Self::CapacityRefused => "plugin refused event capacity preparation",
            Self::NoParameterFlush => "plugin does not accept parameter changes",
            Self::NoWindow => "the plugin has no window",
            Self::NoCocoaWindow => "the plugin has no Cocoa window",
            Self::NoWindowCreate => "the plugin cannot create a window",
            Self::WindowRefused => "the plugin refused to create its window",
            Self::NoEmbedding => "the plugin cannot be embedded",
            Self::ParentRefused => "the plugin refused the window",
            Self::PanelNeedsMainThread => "plugin windows need the main thread",
            Self::PanelHasNoView => "the window has no content view",
            Self::NoAudioPorts => "plugin does not declare audio ports",
            Self::NoPortCount => "plugin audio ports have no count",
            Self::NoPortInfo => "plugin audio ports have no description",
            Self::PortUndescribed(port) => return write!(f, "plugin did not describe {port}"),
            Self::PortWithoutChannels(port) => return write!(f, "{port} has no channels"),
            Self::NoPorts => "plugin declares no audio ports",
            Self::NoNotePortCount => "plugin note ports have no count",
            Self::NoNotePortInfo => "plugin note ports have no description",
            Self::NotePortUndescribed => "plugin rejected a note port description",
            Self::NotePortIndex => "MIDI note port index cannot be represented",
        };
        f.write_str(text)
    }
}

impl std::error::Error for PluginFailure {}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("plugin main thread is busy with an earlier request"),
            Self::NoPlugin => f.write_str("no plugin"),
            Self::TimedOut(Pending::MainCall(call)) => {
                write!(f, "plugin main request {call} timed out; outcome unknown")
            }
            Self::TimedOut(Pending::ParameterChanges) => {
                f.write_str("parameter changes timed out; outcome unknown")
            }
            Self::Stopped(Pending::MainCall(call)) => {
                write!(f, "plugin stopped before main request {call} finished")
            }
            Self::Stopped(Pending::ParameterChanges) => {
                f.write_str("plugin stopped before parameters were applied")
            }
            Self::Plugin(failure) => write!(f, "{failure}"),
            Self::Launch(failure) => write!(f, "{failure}"),
            Self::InvalidParameterId(_) => f.write_str("invalid parameter change"),
            Self::UnexpectedMainReply { expected, .. } => {
                f.write_str(expected.mismatch_diagnostic())
            }
        }
    }
}

impl std::error::Error for Refusal {}

// Kind 0 was the unclassified text refusal. It is deliberately not accepted.
const BUSY: u8 = 1;
const NO_PLUGIN: u8 = 2;
const TIMED_OUT_MAIN: u8 = 3;
const TIMED_OUT_PARAMETERS: u8 = 4;
const STOPPED_MAIN: u8 = 5;
const STOPPED_PARAMETERS: u8 = 6;
/// Followed by the failure's code, then an audio port's side and index for
/// the two port kinds.
const PLUGIN: u8 = 7;
const LAUNCH: u8 = 8;
const INVALID_PARAMETER_ID: u8 = 9;
const UNEXPECTED_MAIN_REPLY: u8 = 10;

pub(crate) fn write_refusal(refusal: &Refusal) -> Vec<u8> {
    let kind = match refusal {
        Refusal::Busy => BUSY,
        Refusal::NoPlugin => NO_PLUGIN,
        Refusal::TimedOut(Pending::ParameterChanges) => TIMED_OUT_PARAMETERS,
        Refusal::Stopped(Pending::ParameterChanges) => STOPPED_PARAMETERS,
        Refusal::TimedOut(Pending::MainCall(call)) => {
            return main_call(TIMED_OUT_MAIN, *call);
        }
        Refusal::Stopped(Pending::MainCall(call)) => return main_call(STOPPED_MAIN, *call),
        Refusal::Plugin(failure) => return plugin_failure(*failure),
        Refusal::Launch(failure) => {
            let mut body = vec![LAUNCH];
            body.extend_from_slice(&failure.encode());
            return body;
        }
        Refusal::InvalidParameterId(id) => {
            let mut body = vec![INVALID_PARAMETER_ID];
            body.extend_from_slice(&id.to_le_bytes());
            return body;
        }
        Refusal::UnexpectedMainReply { expected, actual } => {
            return vec![UNEXPECTED_MAIN_REPLY, *expected as u8, *actual as u8];
        }
    };
    vec![kind]
}

fn main_call(kind: u8, call: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(9);
    body.push(kind);
    body.extend_from_slice(&call.to_le_bytes());
    body
}

fn plugin_failure(failure: PluginFailure) -> Vec<u8> {
    let mut body = vec![PLUGIN, failure_code(failure)];
    if let PluginFailure::PortUndescribed(port) | PluginFailure::PortWithoutChannels(port) = failure
    {
        body.push(u8::from(port.input));
        body.extend_from_slice(&port.index.to_le_bytes());
    }
    body
}

/// The wire code of each failure. The helper and the host ship together, so a
/// code only has to match [`read_plugin_failure`] in the same build.
fn failure_code(failure: PluginFailure) -> u8 {
    match failure {
        PluginFailure::NoState => 0,
        PluginFailure::NoStateSave => 1,
        PluginFailure::NoStateLoad => 2,
        PluginFailure::SaveRefused => 3,
        PluginFailure::LoadRefused => 4,
        PluginFailure::CapacityRefused => 5,
        PluginFailure::NoParameterFlush => 6,
        PluginFailure::NoWindow => 7,
        PluginFailure::NoCocoaWindow => 8,
        PluginFailure::NoWindowCreate => 9,
        PluginFailure::WindowRefused => 10,
        PluginFailure::NoEmbedding => 11,
        PluginFailure::ParentRefused => 12,
        PluginFailure::PanelNeedsMainThread => 13,
        PluginFailure::PanelHasNoView => 14,
        PluginFailure::NoAudioPorts => 15,
        PluginFailure::NoPortCount => 16,
        PluginFailure::NoPortInfo => 17,
        PluginFailure::PortUndescribed(_) => 18,
        PluginFailure::PortWithoutChannels(_) => 19,
        PluginFailure::NoPorts => 20,
        PluginFailure::NoNotePortCount => 21,
        PluginFailure::NoNotePortInfo => 22,
        PluginFailure::NotePortUndescribed => 23,
        PluginFailure::NotePortIndex => 24,
    }
}

fn read_plugin_failure(payload: &[u8]) -> Result<PluginFailure, RefusalDecodeError> {
    let (&code, rest) = payload
        .split_first()
        .ok_or(RefusalDecodeError::MissingPluginKind)?;
    let failure = match code {
        0 => PluginFailure::NoState,
        1 => PluginFailure::NoStateSave,
        2 => PluginFailure::NoStateLoad,
        3 => PluginFailure::SaveRefused,
        4 => PluginFailure::LoadRefused,
        5 => PluginFailure::CapacityRefused,
        6 => PluginFailure::NoParameterFlush,
        7 => PluginFailure::NoWindow,
        8 => PluginFailure::NoCocoaWindow,
        9 => PluginFailure::NoWindowCreate,
        10 => PluginFailure::WindowRefused,
        11 => PluginFailure::NoEmbedding,
        12 => PluginFailure::ParentRefused,
        13 => PluginFailure::PanelNeedsMainThread,
        14 => PluginFailure::PanelHasNoView,
        15 => PluginFailure::NoAudioPorts,
        16 => PluginFailure::NoPortCount,
        17 => PluginFailure::NoPortInfo,
        18 | 19 => {
            let [side, a, b, c, d] = fixed(rest, RefusalField::PluginPayload { kind: code })?;
            let input = match side {
                0 => false,
                1 => true,
                _ => return Err(RefusalDecodeError::InvalidPortSide { kind: code, side }),
            };
            let port = AudioPort {
                input,
                index: u32::from_le_bytes([a, b, c, d]),
            };
            return Ok(if code == 18 {
                PluginFailure::PortUndescribed(port)
            } else {
                PluginFailure::PortWithoutChannels(port)
            });
        }
        20 => PluginFailure::NoPorts,
        21 => PluginFailure::NoNotePortCount,
        22 => PluginFailure::NoNotePortInfo,
        23 => PluginFailure::NotePortUndescribed,
        24 => PluginFailure::NotePortIndex,
        _ => return Err(RefusalDecodeError::UnknownPluginKind(code)),
    };
    fixed::<0>(rest, RefusalField::PluginPayload { kind: code })?;
    Ok(failure)
}

fn fixed<const N: usize>(
    payload: &[u8],
    field: RefusalField,
) -> Result<[u8; N], RefusalDecodeError> {
    payload
        .try_into()
        .map_err(|_| RefusalDecodeError::WrongSize {
            field,
            expected: N,
            actual: payload.len(),
        })
}

/// Reads exactly what [`write_refusal`] writes. Anything else is invalid data,
/// so every refusal the host accepts encodes back to the same bytes.
pub(crate) fn read_refusal(body: &[u8]) -> io::Result<Refusal> {
    let invalid = |error: RefusalDecodeError| io::Error::new(ErrorKind::InvalidData, error);
    let (&kind, payload) = body
        .split_first()
        .ok_or_else(|| invalid(RefusalDecodeError::MissingKind))?;
    let empty = |refusal: Refusal| {
        if payload.is_empty() {
            Ok(refusal)
        } else {
            Err(invalid(RefusalDecodeError::TrailingBytes {
                kind,
                actual: payload.len(),
            }))
        }
    };
    let call = || {
        fixed(payload, RefusalField::MainCall)
            .map(u64::from_le_bytes)
            .map_err(invalid)
    };
    match kind {
        BUSY => empty(Refusal::Busy),
        NO_PLUGIN => empty(Refusal::NoPlugin),
        TIMED_OUT_PARAMETERS => empty(Refusal::TimedOut(Pending::ParameterChanges)),
        STOPPED_PARAMETERS => empty(Refusal::Stopped(Pending::ParameterChanges)),
        TIMED_OUT_MAIN => Ok(Refusal::TimedOut(Pending::MainCall(call()?))),
        STOPPED_MAIN => Ok(Refusal::Stopped(Pending::MainCall(call()?))),
        PLUGIN => read_plugin_failure(payload)
            .map(Refusal::Plugin)
            .map_err(invalid),
        LAUNCH => LaunchFailure::decode(payload)
            .map(Refusal::Launch)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error)),
        INVALID_PARAMETER_ID => fixed(payload, RefusalField::ParameterId)
            .map(u32::from_le_bytes)
            .map(Refusal::InvalidParameterId)
            .map_err(invalid),
        UNEXPECTED_MAIN_REPLY => {
            let [expected, actual] = fixed(payload, RefusalField::MainReplies).map_err(invalid)?;
            let expected = MainReplyKind::decode(expected)
                .ok_or_else(|| invalid(RefusalDecodeError::UnknownExpectedReply(expected)))?;
            let actual = MainReplyKind::decode(actual)
                .ok_or_else(|| invalid(RefusalDecodeError::UnknownActualReply(actual)))?;
            Ok(Refusal::UnexpectedMainReply { expected, actual })
        }
        _ => Err(invalid(RefusalDecodeError::UnknownKind(kind))),
    }
}

#[cfg(test)]
#[path = "refusal_tests.rs"]
mod tests;
