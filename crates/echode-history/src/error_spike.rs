//! Failures specific to the opt-in Phase 0 history benchmark.

use std::fmt;

#[derive(Debug)]
pub enum SpikeError {
    ProjectJson(serde_json::Error),
    TrackJson(serde_json::Error),
    ProjectSchema(u32),
    TrackSchema(u32),
    Stopped,
    TimedOut,
}

impl fmt::Display for SpikeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectJson(error) => write!(f, "project.json: {error}"),
            Self::TrackJson(error) => write!(f, "track json: {error}"),
            Self::ProjectSchema(schema) => write!(f, "unsupported project schema {schema}"),
            Self::TrackSchema(schema) => write!(f, "unsupported track schema {schema}"),
            // Preserve the benchmark's legacy message while separating the causes.
            Self::Stopped | Self::TimedOut => f.write_str("history thread stopped"),
        }
    }
}

impl std::error::Error for SpikeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProjectJson(error) | Self::TrackJson(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::sync::mpsc::RecvTimeoutError> for SpikeError {
    fn from(error: std::sync::mpsc::RecvTimeoutError) -> Self {
        match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => Self::TimedOut,
            std::sync::mpsc::RecvTimeoutError::Disconnected => Self::Stopped,
        }
    }
}

impl From<SpikeError> for crate::PackageError {
    fn from(error: SpikeError) -> Self {
        Self::Spike(error)
    }
}
