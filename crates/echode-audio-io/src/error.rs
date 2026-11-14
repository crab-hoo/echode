//! Why output, audition and device requests fail (ADR 0291).
//!
//! Callers branch on the variant. Text is made only where a report reaches
//! the app, the API or a log, and it keeps the wording those reports had.

use std::fmt;
use std::sync::Arc;

use echode_engine::{PlaybackError, WorkgroupError};
use echode_history::PackageError;

/// Why an output, audition or device request failed.
#[derive(Clone, Debug)]
pub enum AudioError {
    /// The request is not valid. The session did not change for it.
    Rejected(Rejected),
    /// No output device is available.
    NoOutputDevice,
    /// A CoreAudio call returned a failing status.
    CoreAudio { call: CoreAudioCall, status: i32 },
    /// CoreAudio answered with a value this host cannot use.
    Hardware(HardwareFault),
    /// The selected device's workgroup could not be prepared for helpers.
    Workgroup { device: u32, fault: WorkgroupFault },
    /// The open output stopped by itself. It has to open again.
    Stopped(Stopped),
    /// The audition could not open, load or keep sounding.
    Audition(AuditionFault),
    /// The engine refused the request or could not prepare playback.
    Playback(PlaybackError),
    /// A package object, setting or audio file could not be read or written.
    Package(Arc<PackageError>),
    /// A device has recorded plugin state, but the session has no package.
    NoPackage,
    /// A thread of this crate could not start or ended without its result.
    Thread(ThreadFault),
    /// The session's own bookkeeping disagreed with itself.
    Internal(Internal),
}

/// A request this crate refuses before anything changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// No values, an unknown or repeated track, or a gain or pan out of range.
    MixerPreview,
    NegativePosition,
    NegativeStart,
    /// The loop end is not after its start, or the start is negative.
    Loop,
    /// The project has no online plugin device a device view could reach.
    NoRunningPlugin,
    /// The audition gain is not from zero to one.
    AuditionGain,
}

/// The CoreAudio call that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreAudioCall {
    /// Reading a property: the device list, a format, a name or a size.
    Property,
    /// Registering for property changes.
    Listener,
    CreateOutputProc,
    StartOutput,
    CreateAuditionProc,
    StartAudition,
}

/// A CoreAudio answer this host cannot use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareFault {
    /// A property's size does not match its type.
    PropertySize,
    /// A list's size is not whole elements, or it grew past its buffer.
    ListSize,
    /// A list changed between its size and its read on every attempt.
    ListKeptChanging,
    /// The devices changed between enumeration and listening on every attempt.
    DevicesKeptChanging,
    EmptyName,
    ChannelOverflow,
    /// The nominal rate is not a positive whole number.
    SampleRate,
    /// The device does not provide native 32-bit float PCM at its rate.
    Format,
}

/// Why the selected device's workgroup is not ready.
#[derive(Clone, Debug)]
pub enum WorkgroupFault {
    /// `mach_timebase_info` failed.
    Timebase,
    /// The block's period is zero or does not fit a workgroup interval.
    Period,
    /// CoreAudio did not give the device's workgroup.
    Query(Box<AudioError>),
    /// The workgroup could not be retained for helpers.
    Retain(WorkgroupError),
}

/// Why the project output stopped by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The device's workgroup changed under the open output.
    WorkgroupChanged,
    /// The output buffer, format or device changed, or the callback failed.
    DeviceChanged,
    /// Where and why the callback panicked (ADR 0238).
    CallbackPanicked(String),
    /// The driver kept the callback after the output closed (its memory is kept).
    CallbackNotReleased,
}

/// Why the audition did not sound or stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditionFault {
    /// The output device changed under the audition.
    DeviceChanged,
    /// The audition's final device signal became non-finite or exceeded +18 dBFS.
    OutputGuard,
    /// Where and why the audition callback panicked (ADR 0238).
    Panicked(String),
    /// The output closed while the file was loading.
    ClosedWhileLoading,
    /// A sound was ready, but no audition output was open.
    NotOpen,
    /// The file converts to no output samples.
    NoSamples,
}

/// A thread of this crate that did not deliver.
#[derive(Clone, Debug)]
pub enum ThreadFault {
    Spawn(Arc<std::io::Error>),
    /// The output's preparing thread panicked.
    PreparationPanicked,
    /// The output's preparing thread ended without sending a result.
    PreparationStopped,
}

/// A state the session's own steps should never reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    CallbackMissing,
    /// An install closed the output it was installing.
    ClosedWhileOpening,
    /// An opening was installed twice.
    AlreadyInstalled,
}

impl From<Rejected> for AudioError {
    fn from(rejected: Rejected) -> Self {
        Self::Rejected(rejected)
    }
}

impl From<HardwareFault> for AudioError {
    fn from(fault: HardwareFault) -> Self {
        Self::Hardware(fault)
    }
}

impl From<Stopped> for AudioError {
    fn from(stopped: Stopped) -> Self {
        Self::Stopped(stopped)
    }
}

impl From<AuditionFault> for AudioError {
    fn from(fault: AuditionFault) -> Self {
        Self::Audition(fault)
    }
}

impl From<PlaybackError> for AudioError {
    fn from(error: PlaybackError) -> Self {
        Self::Playback(error)
    }
}

impl From<PackageError> for AudioError {
    fn from(error: PackageError) -> Self {
        Self::Package(Arc::new(error))
    }
}

impl From<Internal> for AudioError {
    fn from(internal: Internal) -> Self {
        Self::Internal(internal)
    }
}

impl AudioError {
    pub(crate) fn spawn(error: std::io::Error) -> Self {
        Self::Thread(ThreadFault::Spawn(Arc::new(error)))
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(rejected) => rejected.fmt(formatter),
            Self::NoOutputDevice => formatter.write_str("no output device is available"),
            Self::CoreAudio { call, status } => match call {
                CoreAudioCall::Property | CoreAudioCall::Listener => {
                    write!(formatter, "CoreAudio error {status}")
                }
                CoreAudioCall::CreateOutputProc => {
                    write!(formatter, "could not open the output device ({status})")
                }
                CoreAudioCall::StartOutput => {
                    write!(formatter, "could not start the output device ({status})")
                }
                CoreAudioCall::CreateAuditionProc => {
                    write!(formatter, "could not open the audition output ({status})")
                }
                CoreAudioCall::StartAudition => {
                    write!(formatter, "could not start the audition output ({status})")
                }
            },
            Self::Hardware(fault) => fault.fmt(formatter),
            Self::Workgroup { device, fault } => {
                write!(formatter, "output device {device} workgroup: {fault}")
            }
            Self::Stopped(stopped) => stopped.fmt(formatter),
            Self::Audition(fault) => fault.fmt(formatter),
            Self::Playback(error) => error.fmt(formatter),
            Self::Package(error) => error.fmt(formatter),
            Self::NoPackage => formatter.write_str("plugin state has no package"),
            Self::Thread(fault) => fault.fmt(formatter),
            Self::Internal(internal) => internal.fmt(formatter),
        }
    }
}

impl fmt::Display for Rejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MixerPreview => "invalid mixer preview",
            Self::NegativePosition => "playback position must not be negative",
            Self::NegativeStart => "playback start must not be negative",
            Self::Loop => "loop end must be after its start",
            Self::NoRunningPlugin => "the project has no running plugin",
            Self::AuditionGain => "audition gain must be between zero and one",
        })
    }
}

impl fmt::Display for HardwareFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PropertySize => "CoreAudio property size changed",
            Self::ListSize => "invalid CoreAudio list size",
            Self::ListKeptChanging => "CoreAudio device list kept changing",
            Self::DevicesKeptChanging => "the output devices kept changing",
            Self::EmptyName => "CoreAudio returned an empty name",
            Self::ChannelOverflow => "channel count overflow",
            Self::SampleRate => "CoreAudio sample rate must be a positive whole number",
            Self::Format => "the output device must provide native 32-bit float PCM",
        })
    }
}

impl fmt::Display for WorkgroupFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timebase => formatter.write_str("could not read the timebase"),
            Self::Period => formatter.write_str("period is out of range"),
            Self::Query(error) => error.fmt(formatter),
            Self::Retain(error) => error.fmt(formatter),
        }
    }
}

impl fmt::Display for Stopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkgroupChanged => {
                formatter.write_str("the selected output device workgroup changed")
            }
            Self::DeviceChanged => formatter
                .write_str("audio playback stopped because the output buffer or device changed"),
            Self::CallbackPanicked(reason) => write!(
                formatter,
                "audio playback stopped because the audio callback panicked: {reason}"
            ),
            Self::CallbackNotReleased => {
                formatter.write_str("the audio driver could not release its callback")
            }
        }
    }
}

impl fmt::Display for AuditionFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceChanged => formatter.write_str("the output device changed"),
            Self::OutputGuard => formatter.write_str(
                "sample audition muted: output exceeded +18 dBFS or became non-finite; select a sample to try again",
            ),
            Self::Panicked(reason) => write!(formatter, "the audition callback panicked: {reason}"),
            Self::ClosedWhileLoading => {
                formatter.write_str("the audition output closed during loading")
            }
            Self::NotOpen => formatter.write_str("no audition output is open"),
            Self::NoSamples => formatter.write_str("audio file has no output samples"),
        }
    }
}

impl fmt::Display for ThreadFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => error.fmt(formatter),
            Self::PreparationPanicked => formatter.write_str("output preparation panicked"),
            Self::PreparationStopped => formatter.write_str("output preparation stopped"),
        }
    }
}

impl fmt::Display for Internal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CallbackMissing => "the output callback is missing",
            Self::ClosedWhileOpening => "the output closed while it opened",
            Self::AlreadyInstalled => "the output was already installed",
        })
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Playback(error) => Some(error),
            Self::Package(error) => Some(error.as_ref()),
            Self::Thread(ThreadFault::Spawn(error)) => Some(error.as_ref()),
            Self::Workgroup {
                fault: WorkgroupFault::Query(error),
                ..
            } => Some(error.as_ref()),
            _ => None,
        }
    }
}
