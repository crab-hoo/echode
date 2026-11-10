//! Preserve the loader's cause when the helper refuses its launch handshake.

use crate::HostError;
use clack_host::entry::PluginEntryError;
use clack_host::prelude::PluginInstanceError;
use clack_host::utils::ClapVersion;
use echode_sandbox::{LaunchFailure, LaunchFailureKind, Refusal};

pub(crate) fn launch_refusal(error: &HostError) -> Refusal {
    use LaunchFailureKind as Kind;
    let kind = match error {
        HostError::Plugin(failure) => return Refusal::Plugin(*failure),
        HostError::Entry(error) => entry_kind(error),
        HostError::Instance(error) => instance_kind(*error),
        HostError::NoBinary(_) => Kind::NoBinary,
        HostError::NoFactory(_) => Kind::NoFactory,
        HostError::NoPlugin(_) => Kind::NoPlugin,
        HostError::NoPluginId => Kind::NoPluginId,
        // These are not load/activation failures. None of the launch sites can
        // produce them; keep the helper's own diagnostic if that ever changes.
        HostError::Region(_)
        | HostError::StartRefused
        | HostError::ProcessRefused
        | HostError::Socket(_)
        | HostError::Workgroup(_)
        | HostError::RunLoopSource
        | HostError::NoRunLoop
        | HostError::PortsUnpublished
        | HostError::Panicked(_)
        | HostError::FrameCount(_) => return Refusal::Other(error.to_string()),
    };
    Refusal::Launch(LaunchFailure {
        kind,
        diagnostic: error.to_string(),
    })
}

fn entry_kind(error: &PluginEntryError) -> LaunchFailureKind {
    use LaunchFailureKind as Kind;
    match error {
        PluginEntryError::LibraryLoadingError(_) => Kind::Library,
        PluginEntryError::InvalidNulPath(error) => Kind::InvalidNulPath {
            position: error.nul_position() as u64,
        },
        PluginEntryError::Io(error) => Kind::EntryIo {
            raw_os_error: error.raw_os_error(),
        },
        PluginEntryError::ResolveFailed => Kind::ResolveFailed,
        PluginEntryError::NullEntryPointer => Kind::NullEntryPointer,
        PluginEntryError::IncompatibleClapVersion { plugin_version } => {
            let host = ClapVersion::CURRENT;
            Kind::IncompatibleClapVersion {
                plugin: [
                    plugin_version.major,
                    plugin_version.minor,
                    plugin_version.revision,
                ],
                host: [host.major, host.minor, host.revision],
            }
        }
        PluginEntryError::EntryInitFailed => Kind::EntryInitFailed,
        // The upstream entry enum is non-exhaustive. This preserves a future
        // cause as a launch failure, not an unrelated Refusal::Other.
        _ => Kind::UnknownEntry,
    }
}

fn instance_kind(error: PluginInstanceError) -> LaunchFailureKind {
    use LaunchFailureKind as Kind;
    match error {
        PluginInstanceError::StartProcessingFailed => Kind::StartProcessingFailed,
        PluginInstanceError::AlreadyActivatedPlugin => Kind::AlreadyActivatedPlugin,
        PluginInstanceError::StillActivatedPlugin => Kind::StillActivatedPlugin,
        PluginInstanceError::DeactivatedPlugin => Kind::DeactivatedPlugin,
        PluginInstanceError::ActivationFailed => Kind::ActivationFailed,
        PluginInstanceError::PluginNotFound => Kind::PluginNotFound,
        PluginInstanceError::MissingPluginFactory => Kind::MissingPluginFactory,
        PluginInstanceError::InstantiationFailed => Kind::InstantiationFailed,
        PluginInstanceError::PluginDestroyed => Kind::PluginDestroyed,
        PluginInstanceError::ProcessingFailed => Kind::ProcessingFailed,
        PluginInstanceError::ProcessingStopped => Kind::ProcessingStopped,
        PluginInstanceError::ProcessingStarted => Kind::ProcessingStarted,
        PluginInstanceError::NullFactoryCreatePluginFunction => {
            Kind::NullFactoryCreatePluginFunction
        }
        PluginInstanceError::NullProcessFunction => Kind::NullProcessFunction,
        PluginInstanceError::NullActivateFunction => Kind::NullActivateFunction,
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
