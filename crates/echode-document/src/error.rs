//! Why a document operation failed (ADR 0292).
//!
//! Callers branch on the variant. Text is made only where a completion reaches
//! the C ABI, and it keeps the wording those completions had.

use std::fmt;

use echode_actions::ParameterError;
use echode_audio_io::{AudioError, SessionError};
use echode_history::{HostError, PackageError};
use serde::Serialize;

/// Why a submitted operation, or opening the document, failed.
#[derive(Debug)]
pub enum DocumentError {
    /// The request is not one this document accepts. Nothing changed for it.
    Rejected(Rejected),
    /// The request's JSON did not parse as its operation. Nothing changed.
    Malformed(serde_json::Error),
    /// A single edit's values failed model command validation. Nothing changed.
    Parameters(ParameterError),
    /// The history refused or failed an edit, Undo, Redo, restore, recovery
    /// consent or the revision list.
    History(HostError),
    /// The package could not be opened, still waits for recovery consent, or
    /// an object, export or setting could not be written.
    Package(PackageError),
    /// The audio session refused the request or could not open its output.
    Audio(AudioError),
    /// The device's helper failed the round trip. After
    /// [`SessionError::Timeout`], and after the helper's own timeout or stop
    /// ([`SessionError::Refused`] with `TimedOut` or `Stopped`), the outcome
    /// is unknown: the plugin may still apply the request, and nothing sends
    /// it again (ADR 0199, ADR 0293).
    Helper(SessionError),
    /// A report could not be encoded as JSON.
    Encode(serde_json::Error),
    /// A thread of this document could not start.
    Thread(std::io::Error),
    /// The document stopped before the operation completed.
    Stopped,
    /// A turn panicked. The document refuses every later operation.
    Panicked,
    /// A device round trip panicked. The lane keeps serving later requests.
    DeviceRequestPanicked,
    /// The thread staging an edit's imports panicked. The edit was not
    /// applied, and the document keeps serving later requests (ADR 0331).
    ImportPanicked,
}

/// A request the document refuses before anything changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// An edit with no bytes.
    EmptyCommand,
    /// An edit whose bytes are not UTF-8.
    NotUtf8,
    /// No entry point recognized the operation.
    UnknownOperation,
    /// A history request with an unknown type, or the wrong fields for it.
    UnknownHistoryCommand,
    /// GUI export preferences that are not a JSON string.
    ExportSettings,
    /// An export snapshot without a destination.
    ExportPath,
    /// Recovery consent without the tip it accepts.
    RecoveryTip,
    /// Recovery consent without the base it returns to.
    RecoveryBase,
    /// The device is not in the current project.
    DeviceGone,
    /// The device is the mixer, which has no helper.
    Mixer,
    /// The device is offline, so it has no helper.
    DeviceOffline,
    /// Parameter changes that are empty or not finite.
    ParameterChanges,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(rejected) => write!(f, "{rejected}"),
            Self::Malformed(error) | Self::Encode(error) => write!(f, "{error}"),
            Self::Parameters(error) => write!(f, "{error}"),
            Self::History(error) => write!(f, "{error}"),
            Self::Package(error) => write!(f, "{error}"),
            Self::Audio(error) => write!(f, "{error}"),
            Self::Helper(error) => write!(f, "{error}"),
            Self::Thread(error) => write!(f, "{error}"),
            Self::Stopped => f.write_str("document thread stopped"),
            Self::Panicked => f.write_str("document thread panicked"),
            Self::DeviceRequestPanicked => f.write_str("device request panicked"),
            Self::ImportPanicked => f.write_str("audio import panicked"),
        }
    }
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyCommand => "command is empty",
            Self::NotUtf8 => "command is not utf-8",
            Self::UnknownOperation => "unknown document operation",
            Self::UnknownHistoryCommand => "history command was not recognized",
            Self::ExportSettings => "export settings must be a JSON string",
            Self::ExportPath => "export snapshot path is missing",
            Self::RecoveryTip => "recovery tip is missing",
            Self::RecoveryBase => "recovery base is missing",
            Self::DeviceGone => "the device is no longer in the project",
            Self::Mixer => "the mixer has no plugin parameters",
            Self::DeviceOffline => "the device is offline",
            Self::ParameterChanges => "parameter changes are empty or not finite",
        })
    }
}

impl std::error::Error for DocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Malformed(error) | Self::Encode(error) => Some(error),
            Self::Parameters(error) => Some(error),
            Self::History(error) => Some(error),
            Self::Package(error) => Some(error),
            Self::Audio(error) => Some(error),
            Self::Helper(error) => Some(error),
            Self::Thread(error) => Some(error),
            Self::Rejected(_)
            | Self::Stopped
            | Self::Panicked
            | Self::DeviceRequestPanicked
            | Self::ImportPanicked => None,
        }
    }
}

impl From<Rejected> for DocumentError {
    fn from(rejected: Rejected) -> Self {
        Self::Rejected(rejected)
    }
}

impl From<ParameterError> for DocumentError {
    fn from(error: ParameterError) -> Self {
        match error {
            ParameterError::Json(error) => Self::Malformed(error),
            error => Self::Parameters(error),
        }
    }
}

impl From<HostError> for DocumentError {
    fn from(error: HostError) -> Self {
        Self::History(error)
    }
}

impl From<PackageError> for DocumentError {
    fn from(error: PackageError) -> Self {
        Self::Package(error)
    }
}

impl From<AudioError> for DocumentError {
    fn from(error: AudioError) -> Self {
        Self::Audio(error)
    }
}

impl From<SessionError> for DocumentError {
    fn from(error: SessionError) -> Self {
        Self::Helper(error)
    }
}

/// A report for the app. Encoding one is this crate's own failure, not the
/// request's, so it is not [`DocumentError::Malformed`].
pub(crate) fn encode(report: &impl Serialize) -> Result<Vec<u8>, DocumentError> {
    serde_json::to_vec(report).map_err(DocumentError::Encode)
}
