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
    pub(crate) fn decode(body: &[u8]) -> Option<Self> {
        use LaunchFailureKind::*;
        let (&code, mut rest) = body.split_first()?;
        let kind = match code {
            0 => Library,
            1 => InvalidNulPath {
                position: u64::from_le_bytes(take(&mut rest)?),
            },
            2 => EntryIo {
                raw_os_error: match take::<1>(&mut rest)? {
                    [0] => None,
                    [1] => Some(i32::from_le_bytes(take(&mut rest)?)),
                    _ => return None,
                },
            },
            3 => ResolveFailed,
            4 => NullEntryPointer,
            5 => IncompatibleClapVersion {
                plugin: version(&mut rest)?,
                host: version(&mut rest)?,
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
            _ => return None,
        };
        Some(Self {
            kind,
            diagnostic: std::str::from_utf8(rest).ok()?.to_owned(),
        })
    }
}

fn take<const N: usize>(body: &mut &[u8]) -> Option<[u8; N]> {
    let (head, tail) = body.split_at_checked(N)?;
    *body = tail;
    head.try_into().ok()
}

fn version(body: &mut &[u8]) -> Option<[u32; 3]> {
    Some([
        u32::from_le_bytes(take(body)?),
        u32::from_le_bytes(take(body)?),
        u32::from_le_bytes(take(body)?),
    ])
}

#[cfg(test)]
#[path = "launch_failure_tests.rs"]
mod tests;
