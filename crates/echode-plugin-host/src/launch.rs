//! Preserve the loader's cause when the helper refuses its launch handshake.

use clack_host::entry::PluginEntryError;
use clack_host::prelude::PluginInstanceError;
use clack_host::utils::ClapVersion;
use echode_sandbox::{LaunchFailure, LaunchFailureKind};
use std::path::PathBuf;

/// A failure while loading, creating or activating a plugin, not while serving it.
///
/// Runtime failures cannot enter the launch handshake:
/// ```compile_fail
/// use echode_plugin_host::{HostError, LoadError};
/// let _: LoadError = HostError::StartRefused.into();
/// ```
#[derive(Debug)]
pub enum LoadError {
    Entry(PluginEntryError),
    NoBinary(PathBuf),
    NoFactory(PathBuf),
    NoPlugin(PathBuf),
    NoPluginId,
    Instance(PluginInstanceError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entry(error) => write!(f, "{error}"),
            Self::NoBinary(path) => write!(f, "{} has no plugin binary", path.display()),
            Self::NoFactory(path) => write!(f, "{} has no plugin factory", path.display()),
            Self::NoPlugin(path) => write!(f, "{} does not contain a plugin", path.display()),
            Self::NoPluginId => f.write_str("plugin descriptor has no id"),
            Self::Instance(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Entry(error) => Some(error),
            Self::Instance(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) fn launch_failure(error: &LoadError) -> LaunchFailure {
    use LaunchFailureKind as Kind;
    let kind = match error {
        LoadError::Entry(error) => entry_kind(error),
        LoadError::Instance(error) => instance_kind(*error),
        LoadError::NoBinary(_) => Kind::NoBinary,
        LoadError::NoFactory(_) => Kind::NoFactory,
        LoadError::NoPlugin(_) => Kind::NoPlugin,
        LoadError::NoPluginId => Kind::NoPluginId,
    };
    LaunchFailure {
        kind,
        diagnostic: error.to_string(),
    }
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
        // cause as a launch failure, not an unrelated runtime error.
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
