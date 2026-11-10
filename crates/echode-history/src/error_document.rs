//! Document lifetime, recovery, journal and preference failures.

use std::fmt;

use echode_actions::ActionError;
use echode_model::{InvariantError, ModelError};

use crate::host::RecoveryNotice;

#[derive(Debug)]
pub enum DocumentError {
    AlreadyOpen,
    Stopped,
    NoReadableCommit,
    UnfinishedWorktree,
    RecoveryConfirmationRequired(RecoveryNotice),
    RecoveryMismatch,
    InvalidRevision,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOpen => f.write_str("package is already open"),
            Self::Stopped => f.write_str("document thread stopped"),
            Self::NoReadableCommit => f.write_str("no readable commit"),
            Self::UnfinishedWorktree => {
                f.write_str("worktree save is unfinished and there is no commit")
            }
            Self::RecoveryConfirmationRequired(notice) => {
                f.write_str("recovery confirmation required: ")?;
                match notice.reason {
                    crate::RecoveryReason::UnreadableTip => {
                        write!(f, "unreadable tip {}", notice.tip)?
                    }
                    crate::RecoveryReason::JournalGap => {
                        f.write_str("journal lines after a missing one were not replayed")?
                    }
                    crate::RecoveryReason::MissingState => {
                        f.write_str("a journal line names a missing plugin state")?
                    }
                }
                write!(
                    f,
                    "; recovered base {}; applied journal seq {}. Preserve a copy before continuing",
                    notice.base, notice.applied_seq
                )
            }
            Self::RecoveryMismatch => {
                f.write_str("recovery confirmation does not match this document")
            }
            Self::InvalidRevision => f.write_str("saved revision was not found"),
        }
    }
}

impl std::error::Error for DocumentError {}

#[derive(Debug)]
pub enum JournalError {
    Text(std::str::Utf8Error),
    Decode(ActionError),
    Apply {
        seq: u64,
        error: ModelError,
    },
    Invariants {
        seq: u64,
        error: InvariantError,
    },
    /// Fault injection uses these same typed refusal paths without an OS failure.
    WriteFailed,
    SyncFailed,
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(error) => write!(f, "{error}"),
            Self::Decode(error) => write!(f, "{error}"),
            Self::Apply { seq, error } => {
                write!(f, "journal seq {seq} cannot apply changes: {error}")
            }
            Self::Invariants { seq, error } => {
                write!(f, "journal seq {seq} violates model invariants: {error}")
            }
            Self::WriteFailed => f.write_str("journal write failed"),
            Self::SyncFailed => f.write_str("journal sync failed"),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Text(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Apply { error, .. } => Some(error),
            Self::Invariants { error, .. } => Some(error),
            Self::WriteFailed | Self::SyncFailed => None,
        }
    }
}

#[derive(Debug)]
pub enum SettingsError {
    ExportTooLarge,
    ExportJson(serde_json::Error),
    NegativePlaybackStart,
    TransportJson(serde_json::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportTooLarge => f.write_str("export settings exceed 16 KiB"),
            Self::ExportJson(error) => write!(f, "invalid export settings: {error}"),
            Self::NegativePlaybackStart => f.write_str("playback start must not be negative"),
            Self::TransportJson(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExportJson(error) | Self::TransportJson(error) => Some(error),
            Self::ExportTooLarge | Self::NegativePlaybackStart => None,
        }
    }
}
