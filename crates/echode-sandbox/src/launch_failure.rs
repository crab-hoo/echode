//! A launch cause that can cross the socket without linking the plugin loader
//! into the parent. The original diagnostic is for display only (ADR 0308).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchFailure {
    pub kind: LaunchFailureKind,
    /// The loader's diagnostic, including any path. Never used to classify it.
    pub diagnostic: String,
}

/// Mirrors the loader's causes, not its wording. Native metadata stays typed;
/// only libloading's platform-specific diagnostic is opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchFailureKind {
    Library,
    InvalidNulPath {
        position: u64,
    },
    EntryIo {
        raw_os_error: Option<i32>,
    },
    ResolveFailed,
    NullEntryPointer,
    IncompatibleClapVersion {
        plugin: [u32; 3],
        host: [u32; 3],
    },
    EntryInitFailed,
    /// A future, non-exhaustive clack entry error that this build does not know.
    UnknownEntry,
    NoBinary,
    NoFactory,
    NoPlugin,
    NoPluginId,
    StartProcessingFailed,
    AlreadyActivatedPlugin,
    StillActivatedPlugin,
    DeactivatedPlugin,
    ActivationFailed,
    PluginNotFound,
    MissingPluginFactory,
    InstantiationFailed,
    PluginDestroyed,
    ProcessingFailed,
    ProcessingStopped,
    ProcessingStarted,
    NullFactoryCreatePluginFunction,
    NullProcessFunction,
    NullActivateFunction,
}

/// A fixed-width field in a launch refusal's payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchField {
    Kind,
    NulPosition,
    ErrnoFlag,
    Errno,
    PluginVersion,
    HostVersion,
}

/// Invalid launch-refusal data, not a refusal reported by the plugin loader.
/// Counts are relative to the named field; raw diagnostic bytes are not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchDecodeError {
    Truncated {
        field: LaunchField,
        expected: usize,
        actual: usize,
    },
    UnknownKind(u8),
    InvalidErrnoFlag(u8),
    /// The native error's offsets are relative to the diagnostic text only.
    Diagnostic(std::str::Utf8Error),
}

impl std::fmt::Display for LaunchDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("launch failure is malformed")
    }
}

impl std::error::Error for LaunchDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Diagnostic(error) => Some(error),
            _ => None,
        }
    }
}

impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.diagnostic)
    }
}

impl std::error::Error for LaunchFailure {}

impl LaunchFailure {
    pub(crate) fn encode(&self) -> Vec<u8> {
        use LaunchFailureKind::*;
        let mut body = vec![match self.kind {
            Library => 0,
            InvalidNulPath { .. } => 1,
            EntryIo { .. } => 2,
            ResolveFailed => 3,
            NullEntryPointer => 4,
            IncompatibleClapVersion { .. } => 5,
            EntryInitFailed => 6,
            UnknownEntry => 7,
            NoBinary => 8,
            NoFactory => 9,
            NoPlugin => 10,
            NoPluginId => 11,
            StartProcessingFailed => 12,
            AlreadyActivatedPlugin => 13,
            StillActivatedPlugin => 14,
            DeactivatedPlugin => 15,
            ActivationFailed => 16,
            PluginNotFound => 17,
            MissingPluginFactory => 18,
            InstantiationFailed => 19,
            PluginDestroyed => 20,
            ProcessingFailed => 21,
            ProcessingStopped => 22,
            ProcessingStarted => 23,
            NullFactoryCreatePluginFunction => 24,
            NullProcessFunction => 25,
            NullActivateFunction => 26,
        }];
        match self.kind {
            InvalidNulPath { position } => body.extend_from_slice(&position.to_le_bytes()),
            EntryIo { raw_os_error } => {
                body.push(u8::from(raw_os_error.is_some()));
                if let Some(code) = raw_os_error {
                    body.extend_from_slice(&code.to_le_bytes());
                }
            }
            IncompatibleClapVersion { plugin, host } => {
                for part in plugin.into_iter().chain(host) {
                    body.extend_from_slice(&part.to_le_bytes());
                }
            }
            _ => {}
        }
        body.extend_from_slice(self.diagnostic.as_bytes());
        body
    }

    /// Accept exactly the writer's format, so accepted messages encode back to
    /// identical bytes. Short metadata and non-UTF-8 diagnostics are invalid.
    pub(crate) fn decode(body: &[u8]) -> Result<Self, LaunchDecodeError> {
        use LaunchFailureKind::*;
        let mut rest = body;
        let [code] = take(&mut rest, LaunchField::Kind)?;
        let kind = match code {
            0 => Library,
            1 => InvalidNulPath {
                position: u64::from_le_bytes(take(&mut rest, LaunchField::NulPosition)?),
            },
            2 => EntryIo {
                raw_os_error: match take(&mut rest, LaunchField::ErrnoFlag)? {
                    [0] => None,
                    [1] => Some(i32::from_le_bytes(take(&mut rest, LaunchField::Errno)?)),
                    [flag] => return Err(LaunchDecodeError::InvalidErrnoFlag(flag)),
                },
            },
            3 => ResolveFailed,
            4 => NullEntryPointer,
            5 => IncompatibleClapVersion {
                plugin: version(&mut rest, LaunchField::PluginVersion)?,
                host: version(&mut rest, LaunchField::HostVersion)?,
            },
            6 => EntryInitFailed,
            7 => UnknownEntry,
            8 => NoBinary,
            9 => NoFactory,
            10 => NoPlugin,
            11 => NoPluginId,
            12 => StartProcessingFailed,
            13 => AlreadyActivatedPlugin,
            14 => StillActivatedPlugin,
            15 => DeactivatedPlugin,
            16 => ActivationFailed,
            17 => PluginNotFound,
            18 => MissingPluginFactory,
            19 => InstantiationFailed,
            20 => PluginDestroyed,
            21 => ProcessingFailed,
            22 => ProcessingStopped,
            23 => ProcessingStarted,
            24 => NullFactoryCreatePluginFunction,
            25 => NullProcessFunction,
            26 => NullActivateFunction,
            _ => return Err(LaunchDecodeError::UnknownKind(code)),
        };
        Ok(Self {
            kind,
            diagnostic: std::str::from_utf8(rest)
                .map_err(LaunchDecodeError::Diagnostic)?
                .to_owned(),
        })
    }
}

fn take<const N: usize>(
    body: &mut &[u8],
    field: LaunchField,
) -> Result<[u8; N], LaunchDecodeError> {
    let (head, tail) = body
        .split_at_checked(N)
        .ok_or(LaunchDecodeError::Truncated {
            field,
            expected: N,
            actual: body.len(),
        })?;
    let mut bytes = [0; N];
    bytes.copy_from_slice(head);
    *body = tail;
    Ok(bytes)
}

fn version(body: &mut &[u8], field: LaunchField) -> Result<[u32; 3], LaunchDecodeError> {
    let mut version = [0; 3];
    for (part, chunk) in version
        .iter_mut()
        .zip(take::<12>(body, field)?.chunks_exact(4))
    {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(chunk);
        *part = u32::from_le_bytes(bytes);
    }
    Ok(version)
}

#[cfg(test)]
#[path = "launch_failure_tests.rs"]
mod tests;
