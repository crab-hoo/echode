//! Errors at this crate's boundary (M1, ADR 0294). Each cause is a variant, so
//! callers branch on the cause and text is made only where it is shown.

use std::fmt;
use std::sync::Arc;

use echode_sandbox::{RegionError, SessionError, WorkgroupError};

/// Why playback could not step, prepare or reach a device.
#[derive(Clone, Debug)]
pub enum PlaybackError {
    InvalidSampleRate,
    InvalidLoop,
    InvalidFader,
    Time,
    BufferTooSmall,
    IdSpace,
    InvalidDevice,
    /// The plugin preparation thread panicked or ended without its result.
    Plugin,
    /// The owner of a preparation stopped it before every helper started, for
    /// example because its output closed. Started helpers were stopped (ADR 0334).
    Cancelled,
    Media,
    /// An audio placement's content is neither mono nor stereo.
    ClipChannels,
    /// The device's main ports are not a mono/stereo layout this host can run.
    BusMapping {
        device: u128,
        clap_id: String,
    },
    /// The helper for an installed plugin could not start or load it.
    Spawn {
        device: u128,
        clap_id: String,
        error: Arc<SessionError>,
    },
    /// A running helper failed or refused a request: a state load or a new
    /// mapping. A refusal tells busy and an unknown outcome apart (ADR 0293).
    Helper {
        device: u128,
        error: Arc<SessionError>,
    },
    /// The recorded state a new or changed helper must load could not be
    /// read. The reader's error is the cause (ADR 0329).
    State {
        device: u128,
        error: crate::StateReadError,
    },
    /// The region the helpers write their output into could not be created.
    FanIn(Arc<RegionError>),
    /// The output device's workgroup could not be handed to helpers.
    Workgroup(WorkgroupError),
    /// A preparing thread could not start.
    Thread(Arc<std::io::Error>),
    /// The device is offline, unresolved or not yet published, so no helper
    /// runs it.
    NoRunningPlugin,
    /// Cached sources were staged while the transport played or had commands
    /// in flight.
    TransportBusy,
    /// An offline render met an online plugin device. Only the isolated
    /// plugin host renders those.
    NeedsPluginHost,
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSampleRate => "sample rate must be above zero",
            Self::InvalidLoop => "loop end must be after its start",
            Self::InvalidFader => "pan must be from -1 to 1 and gain must be finite",
            Self::Time => "playback position overflowed",
            Self::BufferTooSmall => "the caller buffer cannot hold this block",
            Self::IdSpace => "playback note numbers are exhausted",
            Self::InvalidDevice => "device state or parameter is not valid",
            Self::Plugin => "plugin helper did not start",
            Self::Cancelled => "plugin preparation was cancelled",
            Self::Media => "audio file could not be read",
            Self::ClipChannels => "audio placement requires a mono or stereo layout",
            Self::BusMapping { device, clap_id } => {
                return write!(
                    formatter,
                    "device {device} ({clap_id}): unsupported mono/stereo bus mapping"
                );
            }
            Self::Spawn {
                device,
                clap_id,
                error,
            } => return write!(formatter, "device {device} ({clap_id}): {error}"),
            Self::Helper { error, .. } => return error.fmt(formatter),
            Self::State { device, error } => {
                return write!(
                    formatter,
                    "device {device}: saved plugin state could not be read: {error}"
                );
            }
            Self::FanIn(error) => return error.fmt(formatter),
            Self::Workgroup(error) => return error.fmt(formatter),
            Self::Thread(error) => return error.fmt(formatter),
            Self::NoRunningPlugin => "the device has no running plugin",
            Self::TransportBusy => "cached sources require an idle transport",
            Self::NeedsPluginHost => "device rendering requires the isolated plugin host",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PlaybackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { error, .. } | Self::Helper { error, .. } => Some(error.as_ref()),
            Self::State { error, .. } => Some(error.as_ref()),
            Self::FanIn(error) => Some(error.as_ref()),
            Self::Workgroup(error) => Some(error),
            Self::Thread(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<echode_dsp::DspError> for PlaybackError {
    fn from(error: echode_dsp::DspError) -> Self {
        match error {
            echode_dsp::DspError::InvalidSampleRate => Self::InvalidSampleRate,
            echode_dsp::DspError::BufferTooSmall => Self::BufferTooSmall,
            echode_dsp::DspError::Time => Self::Time,
            echode_dsp::DspError::InvalidState => Self::InvalidDevice,
        }
    }
}

impl From<WorkgroupError> for PlaybackError {
    fn from(error: WorkgroupError) -> Self {
        Self::Workgroup(error)
    }
}

impl PlaybackError {
    pub(crate) fn helper(device: u128, error: SessionError) -> Self {
        Self::Helper {
            device,
            error: Arc::new(error),
        }
    }

    pub(crate) fn thread(error: std::io::Error) -> Self {
        Self::Thread(Arc::new(error))
    }
}

/// The device and cause of a rejected export block. Constructed on the control
/// side from the audio thread's existing failure code, never on that callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginBlockFailure {
    pub device: u128,
    pub clap_id: String,
    pub reason: PluginBlockFailureReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginBlockFailureReason {
    ProcessRefused,
    Deadline,
    Submit,
    NonFinite,
    Exited,
}

impl fmt::Display for PluginBlockFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.reason {
            PluginBlockFailureReason::ProcessRefused => "process refused",
            PluginBlockFailureReason::Deadline => "did not finish before the deadline",
            PluginBlockFailureReason::Submit => "could not submit audio",
            PluginBlockFailureReason::NonFinite => "returned a NaN or an infinity",
            PluginBlockFailureReason::Exited => "plugin process has exited",
        };
        write!(f, "device {} ({}): {reason}", self.device, self.clap_id)
    }
}

impl std::error::Error for PluginBlockFailure {}

/// Why a CoreAudio query of the default output failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// A property call returned a failing status.
    Property {
        property: DeviceProperty,
        status: i32,
    },
    /// The device answered without an IO workgroup.
    NoWorkgroup,
}

/// The default output's property that was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceProperty {
    DefaultOutput,
    SampleRate,
    BufferFrames,
    Workgroup,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Property { property, status } => {
                let name = match property {
                    DeviceProperty::DefaultOutput => "default output device",
                    DeviceProperty::SampleRate => "sample rate",
                    DeviceProperty::BufferFrames => "buffer size",
                    DeviceProperty::Workgroup => "workgroup",
                };
                write!(formatter, "{name}: {status}")
            }
            Self::NoWorkgroup => formatter.write_str("device did not return an IO workgroup"),
        }
    }
}

impl std::error::Error for DeviceError {}

impl DeviceProperty {
    pub(crate) fn failed(self, status: i32) -> DeviceError {
        DeviceError::Property {
            property: self,
            status,
        }
    }
}
