//! Errors at this crate's boundary (M1, ADR 0293). Each cause is a variant, so
//! callers branch on the cause and text is made only where it is shown.

use crate::refusal::Refusal;
use crate::region::RegionError;
use std::ffi::OsString;
use std::process::ExitStatus;

#[derive(Debug)]
pub enum SessionError {
    Region(RegionError),
    Io(std::io::Error),
    /// The helper did not answer before this side's deadline. The outcome is
    /// unknown (ADR 0199).
    Timeout,
    /// The owner stopped waiting for the helper to start, and the helper was
    /// stopped (ADR 0334).
    Cancelled,
    Missed,
    MappingPending,
    HostExited {
        status: ExitStatus,
        stderr: String,
    },
    /// The helper has exited. The audio thread reports this without waiting
    /// for the process or reading its stderr.
    HelperDown,
    /// The plugin returned an error from `process`. Not a deadline miss.
    ProcessRefused,
    /// The device workgroup could not be handed to the helper.
    Workgroup(WorkgroupError),
    /// The helper process could not be started.
    Spawn(SpawnError),
    /// The helper answered without the request's result. Busy, a timeout and
    /// a stop are told apart by [`Refusal`].
    Refused(Refusal),
    /// The helper refused the number of chain stages.
    ChainRefused,
    /// The helper kept its previous mapping.
    RemapRefused,
    /// A background save already waits for its reply. Nothing was sent.
    SavePending,
    /// A parameter value is NaN or infinite. Nothing was sent.
    NonFiniteParameter,
    /// The helper broke the control protocol, or this side lost a request.
    Protocol(ProtocolError),
    /// Starting the helper failed, and the helper wrote why on stderr.
    Launch {
        error: Box<SessionError>,
        stderr: String,
    },
}

impl SessionError {
    /// The start was cancelled by its owner, whether or not the helper wrote
    /// to stderr first.
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled => true,
            Self::Launch { error, .. } => error.is_cancelled(),
            _ => false,
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Region(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Timeout => f.write_str("plugin process did not answer before the deadline"),
            Self::Cancelled => f.write_str("plugin process start was cancelled"),
            Self::MappingPending => f.write_str("prepared mapping has not been activated"),
            Self::Missed => f.write_str("both block slots are still in flight"),
            Self::Workgroup(error) => write!(f, "{error}"),
            Self::Spawn(error) => write!(f, "{error}"),
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::ChainRefused => f.write_str("helper rejected the chain"),
            Self::RemapRefused => f.write_str("helper kept the previous mapping"),
            Self::SavePending => f.write_str("a background save is already pending"),
            Self::NonFiniteParameter => f.write_str("parameter value is not finite"),
            Self::Protocol(error) => write!(f, "{error}"),
            Self::HostExited { status, stderr } => {
                write!(f, "plugin process exited ({status})")?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            Self::HelperDown => f.write_str("plugin process has exited"),
            Self::ProcessRefused => f.write_str("process refused"),
            Self::Launch { error, stderr } => write!(f, "{error}: {stderr}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<RegionError> for SessionError {
    fn from(error: RegionError) -> Self {
        match error {
            RegionError::Wait(crate::sync::WaitError::Timeout) => Self::Timeout,
            RegionError::Busy => Self::Missed,
            other => Self::Region(other),
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Refusal> for SessionError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<ProtocolError> for SessionError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

/// The helper broke the control protocol, or this side lost track of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    /// A reply of another kind answered the request. Names what the reply
    /// should have done ("return state").
    UnexpectedReply(&'static str),
    /// The declared audio ports have more channels than `u32` counts.
    ChannelCountOverflow,
    /// A queued message was gone when this side took it.
    QueueLost,
    /// Every request id has been used.
    RequestIdsExhausted,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedReply(what) => write!(f, "helper did not {what}"),
            Self::ChannelCountOverflow => f.write_str("audio port channel count overflowed"),
            Self::QueueLost => f.write_str("queued control message disappeared"),
            Self::RequestIdsExhausted => f.write_str("control request ids exhausted"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// A device workgroup could not be retained, passed or joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkgroupError {
    /// The device gave a null workgroup or a zero period.
    InvalidDevice,
    /// A handoff has a null port or a zero period.
    InvalidHandoff,
    /// A null workgroup was passed in.
    NullWorkgroup,
    /// A null port was to be sent.
    NullPort,
    /// The bootstrap name contains a nul byte.
    NulInName,
    /// A Mach or workgroup call returned this status.
    Call { call: WorkgroupCall, status: i32 },
    /// `bootstrap_check_in` failed or gave no port.
    CheckIn { status: i32, port: u32 },
    /// The received message did not carry exactly one port.
    Message { count: u32, port: u32 },
    /// `os_workgroup_create_with_port` returned null.
    Create,
    /// The thread could not take the device period as its time constraint.
    ThreadPolicy { period_ticks: u32 },
}

/// The call behind [`WorkgroupError::Call`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkgroupCall {
    CopyPort,
    Join,
    LookUp,
    Send,
    Receive,
}

impl std::fmt::Display for WorkgroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDevice => f.write_str("device workgroup or period is invalid"),
            Self::InvalidHandoff => f.write_str("workgroup port or period is invalid"),
            Self::NullWorkgroup => f.write_str("workgroup is null"),
            Self::NullPort => f.write_str("workgroup port is null"),
            Self::NulInName => f.write_str("workgroup name contains a nul"),
            Self::Call { call, status } => match call {
                WorkgroupCall::CopyPort => write!(f, "os_workgroup_copy_port returned {status}"),
                WorkgroupCall::Join => write!(f, "os_workgroup_join returned {status}"),
                WorkgroupCall::LookUp => write!(f, "bootstrap_look_up returned {status}"),
                WorkgroupCall::Send => write!(f, "mach_msg send returned {status:#x}"),
                WorkgroupCall::Receive => write!(f, "mach_msg receive returned {status:#x}"),
            },
            Self::CheckIn { status, port } => {
                write!(f, "bootstrap_check_in returned {status}, port {port}")
            }
            Self::Message { count, port } => {
                write!(f, "workgroup message had count {count} and port {port}")
            }
            Self::Create => f.write_str("os_workgroup_create_with_port returned null"),
            Self::ThreadPolicy { period_ticks } => write!(
                f,
                "thread_policy_set failed for a period of {period_ticks} ticks"
            ),
        }
    }
}

impl std::error::Error for WorkgroupError {}

/// The helper process could not be started.
#[derive(Debug)]
pub enum SpawnError {
    /// The helper's path contains a nul byte.
    NulInPath,
    /// An argument contains a nul byte.
    NulInArgument(OsString),
    /// The added environment contains a nul byte.
    NulInEnvironment,
    /// The stderr pipe could not be made.
    Pipe(std::io::Error),
    /// The stderr pipe could not be moved above the standard descriptors.
    DupPipe(std::io::Error),
    /// A `posix_spawn` step returned this error number.
    Posix { step: SpawnStep, code: i32 },
    /// The helper started without the read end of its stderr pipe.
    NoStderrPipe,
}

/// The step behind [`SpawnError::Posix`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnStep {
    FileActions,
    Attributes,
    CloseOthers,
    Stdin,
    Stdout,
    Stderr,
    CloseStderrRead,
    CloseStderrWrite,
    Inherit { from: i32, to: i32 },
    CloseInherited(i32),
    Spawn,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NulInPath => f.write_str("host path contains a nul"),
            Self::NulInArgument(argument) => write!(f, "argument contains a nul: {argument:?}"),
            Self::NulInEnvironment => f.write_str("environment contains a nul"),
            Self::Pipe(error) => write!(f, "pipe: {error}"),
            Self::DupPipe(error) => write!(f, "dup pipe: {error}"),
            Self::Posix { step, code } => write!(f, "{step}: {code}"),
            Self::NoStderrPipe => f.write_str("stderr pipe was not created"),
        }
    }
}

impl std::fmt::Display for SpawnStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileActions => f.write_str("posix_spawn_file_actions_init"),
            Self::Attributes => f.write_str("posix_spawnattr_init"),
            Self::CloseOthers => f.write_str("close other descriptors"),
            Self::Stdin => f.write_str("redirect stdin"),
            Self::Stdout => f.write_str("redirect stdout"),
            Self::Stderr => f.write_str("redirect stderr"),
            Self::CloseStderrRead => f.write_str("close stderr read end"),
            Self::CloseStderrWrite => f.write_str("close stderr write end"),
            Self::Inherit { from, to } => write!(f, "inherit fd {from} -> {to}"),
            Self::CloseInherited(fd) => write!(f, "close inherited fd {fd}"),
            Self::Spawn => f.write_str("posix_spawn"),
        }
    }
}

impl std::error::Error for SpawnError {}
