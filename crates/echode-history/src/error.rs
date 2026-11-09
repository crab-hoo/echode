//! Typed storage failures. Git, schema and document errors are migrated separately.

use std::collections::TryReserveError;
use std::fmt;
use std::path::PathBuf;

/// A package or journal could not be read or written.
#[derive(Debug)]
pub enum PackageError {
    /// Package, Git and document paths not migrated to typed causes yet.
    Message(String),
    /// A journal write reached the file but its durability is unknown.
    JournalDurabilityUnknown,
    Io {
        path: Option<PathBuf>,
        error: std::io::Error,
    },
    Audio(MediaError),
    MediaIndex(serde_json::Error),
    InvalidObjectHash,
    ObjectHashMismatch {
        hash: String,
    },
    InvalidPeakHash,
}

impl PackageError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub(crate) fn durability_unknown() -> Self {
        Self::JournalDurabilityUnknown
    }

    /// The journal line reached the file and a later sync or rollback did not
    /// finish. Later edits stop until the package is opened again (AUD-03).
    pub fn is_durability_unknown(&self) -> bool {
        matches!(self, Self::JournalDurabilityUnknown)
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::JournalDurabilityUnknown => {
                f.write_str(crate::document::JOURNAL_DURABILITY_UNKNOWN)
            }
            Self::Io { path, error } => {
                if let Some(path) = path {
                    write!(f, "{}: ", path.display())?;
                }
                write!(f, "{error}")
            }
            Self::Audio(error) => write!(f, "{error}"),
            Self::MediaIndex(error) => write!(f, "media index: {error}"),
            Self::InvalidObjectHash => f.write_str("object hash must be 64 lowercase hex digits"),
            Self::ObjectHashMismatch { hash } => {
                write!(f, "object {hash} does not match its BLAKE3")
            }
            Self::InvalidPeakHash => f.write_str("peak cache name is not a hash"),
        }
    }
}

impl std::error::Error for PackageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Audio(error) => Some(error),
            Self::MediaIndex(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for PackageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io { path: None, error }
    }
}

impl From<MediaError> for PackageError {
    fn from(error: MediaError) -> Self {
        Self::Audio(error)
    }
}

/// Canonical PCM validation and AudioToolbox decoder failures.
#[derive(Debug)]
pub enum MediaError {
    FrameCountOverflow,
    SampleCountMismatch,
    NotCanonicalPcm,
    UnusableHeader,
    TooLarge,
    ByteCountOverflow,
    LengthMismatch,
    NonFiniteSample,
    SeekOverflow,
    SeekPastEnd,
    Seek(i32),
    UnsupportedChannels(u32),
    NoSamples,
    EmptyRead,
    ReadTooLarge,
    Read(i32),
    DecoderOverflow,
    Cancelled,
    Allocation(TryReserveError),
    NotFileUrl,
    Open(i32),
    InvalidSampleRate(f64),
    SampleRateTooHigh(f64),
    Property {
        property: u32,
        status: i32,
        expected_size: u32,
        actual_size: u32,
    },
    SetProperty {
        property: u32,
        status: i32,
    },
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameCountOverflow => f.write_str("audio frame count overflowed"),
            Self::SampleCountMismatch => f.write_str("audio sample count does not match"),
            Self::NotCanonicalPcm => f.write_str("audio object is not canonical PCM"),
            Self::UnusableHeader => f.write_str("audio object header is unusable"),
            Self::TooLarge => f.write_str("audio is too large"),
            Self::ByteCountOverflow => f.write_str("audio byte count overflowed"),
            Self::LengthMismatch => f.write_str("audio object length does not match"),
            Self::NonFiniteSample => f.write_str("audio contains a non-finite sample"),
            Self::SeekOverflow => f.write_str("audio seek overflowed"),
            Self::SeekPastEnd => f.write_str("audio seek exceeds the file"),
            Self::Seek(status) => write!(f, "could not seek audio ({status})"),
            Self::UnsupportedChannels(_) => {
                f.write_str("only mono and stereo audio can be imported")
            }
            Self::NoSamples => f.write_str("audio file has no samples"),
            Self::EmptyRead => f.write_str("audio read must request frames"),
            Self::ReadTooLarge => f.write_str("audio read is too large"),
            Self::Read(status) => write!(f, "could not read audio ({status})"),
            Self::DecoderOverflow => f.write_str("audio decoder exceeded its buffer"),
            Self::Cancelled => f.write_str("audio decoding was cancelled"),
            Self::Allocation(_) => f.write_str("audio is too large to decode"),
            Self::NotFileUrl => f.write_str("audio path is not a file URL"),
            Self::Open(status) => write!(f, "could not open audio ({status})"),
            Self::InvalidSampleRate(_) => {
                f.write_str("audio sample rate is not a positive integer")
            }
            Self::SampleRateTooHigh(_) => f.write_str("audio sample rate is too high"),
            Self::Property { status, .. } => {
                write!(f, "could not read an audio property ({status})")
            }
            Self::SetProperty { status, .. } => {
                write!(f, "could not set the audio client format ({status})")
            }
        }
    }
}

impl std::error::Error for MediaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Allocation(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
