//! Command failures retain their causes until the executable prints them.
use std::fmt;
use std::io;
use std::num::{ParseFloatError, ParseIntError, TryFromIntError};
use std::path::PathBuf;

/// Why a command stopped. Display text and exit status are independent of its cause.
#[derive(Debug)]
pub enum CliError {
    Argument(ArgumentError),
    Io {
        operation: CliIoOperation,
        path: Option<PathBuf>,
        error: io::Error,
    },
    Api(Box<echode_api::ApiError>),
    Package(Box<echode_history::PackageError>),
    Host(Box<echode_history::HostError>),
    Audio(Box<echode_audio_io::AudioError>),
    Playback(echode_engine::PlaybackError),
    PluginBlock(echode_engine::PluginBlockFailure),
    MissingPluginHost,
    MissingPlugins(Vec<String>),
    PluginPlaybackFailed,
    /// A plugin kept changing its latency, so no drawing of the export
    /// stayed aligned from its first frame to its last (ADR 0340).
    UnsettledLatency,
    InfiniteTail,
    ExportLengthOverflow,
    WavStereo,
    WavFrameCount(TryFromIntError),
    WavByteRate {
        sample_rate: u32,
        frame_bytes: u32,
    },
    WavFileName,
    NonFinitePcm {
        bits: u16,
    },
    NonFiniteNormalized,
    /// Fault injection before publication; there is no underlying OS error.
    WavWriteFailed,
    WavFlushFailed,
    WavRenameFailed,
    /// Rename succeeded. The finished output must remain, even after cancellation.
    DurabilityUnknown {
        path: PathBuf,
        error: Option<io::Error>,
    },
    Cancelled,
}

/// Invalid command input, distinct from a failed render or filesystem operation.
#[derive(Debug)]
pub enum ArgumentError {
    Usage,
    SampleRate,
    ConflictingLength,
    WavFormat(String),
    Dither(String),
    DitherFormat,
    Integer {
        text: String,
        error: ParseIntError,
    },
    PeakTarget {
        text: String,
        error: Option<ParseFloatError>,
    },
    OutputFileName,
    OutputInsidePackage,
}

/// The filesystem or thread operation that failed, without parsing its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliIoOperation {
    ResolvePackage,
    ResolveOutputDirectory,
    MonitorCancellation,
    CreateWav,
    WriteWavHeader,
    WriteWavSamples,
    FlushWav,
    SyncWav,
    RenameWav,
    CreateScratch,
    UnlinkScratch,
    WriteScratch,
    FlushScratch,
    RewindScratch,
    ReadScratch,
}

impl CliError {
    pub(crate) fn io(
        operation: CliIoOperation,
        path: impl Into<Option<PathBuf>>,
        error: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            error,
        }
    }

    /// Cancellation was acknowledged before publishing the output file.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// The finished file is on disk, but the directory sync after rename failed.
    pub fn is_durability_unknown(&self) -> bool {
        matches!(self, Self::DurabilityUnknown { .. })
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Argument(error) => return error.fmt(f),
            Self::Io {
                operation, error, ..
            } => {
                let prefix = match operation {
                    CliIoOperation::ResolvePackage => "cannot resolve project package: ",
                    CliIoOperation::ResolveOutputDirectory => {
                        "cannot resolve export output directory: "
                    }
                    CliIoOperation::MonitorCancellation => "cannot monitor export cancellation: ",
                    CliIoOperation::CreateScratch
                    | CliIoOperation::UnlinkScratch
                    | CliIoOperation::WriteScratch
                    | CliIoOperation::FlushScratch
                    | CliIoOperation::RewindScratch
                    | CliIoOperation::ReadScratch => "normalization scratch I/O failed: ",
                    _ => "",
                };
                return write!(f, "{prefix}{error}");
            }
            Self::Api(error) => return error.fmt(f),
            Self::Package(error) => return error.fmt(f),
            Self::Host(error) => return error.fmt(f),
            Self::Audio(error) => return error.fmt(f),
            Self::Playback(error) => return error.fmt(f),
            Self::PluginBlock(error) => return error.fmt(f),
            Self::MissingPluginHost => "plugin host is missing",
            Self::MissingPlugins(ids) => return write!(f, "missing plugin: {}", ids.join(", ")),
            Self::PluginPlaybackFailed => "plugin playback failed",
            Self::UnsettledLatency => "a plugin kept changing its latency during the export",
            Self::InfiniteTail => "infinite plugin tail; specify --tail-frames or --frames",
            Self::ExportLengthOverflow => "export length overflowed",
            Self::WavStereo => "wav data must be stereo",
            Self::WavFrameCount(_) => "wav frame count exceeds the renderer's u32 range",
            Self::WavByteRate { .. } => "sample rate does not fit in the wav header",
            Self::WavFileName => "wav path has no file name",
            Self::NonFinitePcm { bits } => {
                return write!(f, "non-finite sample in PCM{bits} export");
            }
            Self::NonFiniteNormalized => "non-finite sample in peak-normalized export",
            Self::WavWriteFailed => "wav write failed",
            Self::WavFlushFailed => "wav flush failed",
            Self::WavRenameFailed => "wav rename failed",
            Self::DurabilityUnknown { error, .. } => {
                f.write_str("export durability is unknown")?;
                if let Some(error) = error {
                    write!(f, ": {error}")?;
                }
                return Ok(());
            }
            Self::Cancelled => "export cancelled",
        };
        f.write_str(message)
    }
}

impl fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Usage => {
                "usage: echode render PACKAGE WAV [--sample-rate 48000] [--frames N | --tail-frames N] [--format float32|pcm16|pcm24] [--dither none|tpdf] [--normalize-peak-db DB] [--progress-json] [--cancel-stdin]\n       echode serve PACKAGE SOCKET\n       echode mcp SOCKET"
            }
            Self::SampleRate => "invalid sample rate",
            Self::ConflictingLength => "cannot combine --frames and --tail-frames",
            Self::WavFormat(text) => {
                return write!(
                    f,
                    "expected wav format float32, pcm16, or pcm24, got {text}"
                );
            }
            Self::Dither(text) => return write!(f, "expected dither none or tpdf, got {text}"),
            Self::DitherFormat => "--dither tpdf requires pcm16 or pcm24",
            Self::Integer { text, .. } => {
                return write!(f, "expected a non-negative integer, got {text}");
            }
            Self::PeakTarget { .. } => "peak target must be a finite dBFS value from -120 to 0",
            Self::OutputFileName => "export output must name a file",
            Self::OutputInsidePackage => "choose an output outside the project package",
        };
        f.write_str(message)
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Argument(error) => Some(error),
            Self::Io { error, .. }
            | Self::DurabilityUnknown {
                error: Some(error), ..
            } => Some(error),
            Self::Api(error) => Some(error.as_ref()),
            Self::Package(error) => Some(error.as_ref()),
            Self::Host(error) => Some(error.as_ref()),
            Self::Audio(error) => Some(error.as_ref()),
            Self::Playback(error) => Some(error),
            Self::PluginBlock(error) => Some(error),
            Self::WavFrameCount(error) => Some(error),
            _ => None,
        }
    }
}

impl std::error::Error for ArgumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Integer { error, .. } => Some(error),
            Self::PeakTarget {
                error: Some(error), ..
            } => Some(error),
            _ => None,
        }
    }
}

impl From<ArgumentError> for CliError {
    fn from(error: ArgumentError) -> Self {
        Self::Argument(error)
    }
}
impl From<echode_api::ApiError> for CliError {
    fn from(error: echode_api::ApiError) -> Self {
        Self::Api(Box::new(error))
    }
}
impl From<echode_history::PackageError> for CliError {
    fn from(error: echode_history::PackageError) -> Self {
        Self::Package(Box::new(error))
    }
}
impl From<echode_history::HostError> for CliError {
    fn from(error: echode_history::HostError) -> Self {
        Self::Host(Box::new(error))
    }
}
impl From<echode_audio_io::AudioError> for CliError {
    fn from(error: echode_audio_io::AudioError) -> Self {
        Self::Audio(Box::new(error))
    }
}
impl From<echode_engine::PlaybackError> for CliError {
    fn from(error: echode_engine::PlaybackError) -> Self {
        Self::Playback(error)
    }
}
