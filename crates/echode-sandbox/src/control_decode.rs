//! Structural failures in control metadata, before a message exists.

/// A control field. Sizes count only this field's bytes, not the frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlField {
    MessageType,
    RequestId,
    RestartLatency,
    ChainStages,
    PortTailLatency,
    PortCount {
        input: bool,
    },
    PortChannels {
        input: bool,
    },
    RemapLayout,
    /// A message whose body must be empty (identified by its wire type).
    EmptyBody {
        message_type: u32,
    },
    PluginCommand,
}

/// Why control metadata is invalid. Raw payload bytes are not retained.
/// Text and refusal payloads keep their own error types instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlDecodeError {
    MessageLength {
        actual: usize,
        minimum: usize,
        maximum: usize,
    },
    /// Exact size for fixed fields; the required minimum for truncated fields.
    WrongSize {
        field: ControlField,
        expected: usize,
        actual: usize,
    },
    PortCountOverflow {
        input: bool,
        count: u32,
    },
    TrailingPortBytes {
        actual: usize,
    },
    MissingMapping,
    UnknownMessageType(u32),
    UnknownPluginCommand(u8),
}

impl std::fmt::Display for ControlDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ControlField::*;
        f.write_str(match self {
            Self::MessageLength { .. } => "control message length is not usable",
            Self::WrongSize { field, .. } => match field {
                MessageType => "control message has no type",
                RequestId => "control message has no request id",
                RestartLatency => "restart notice is the wrong size",
                ChainStages => "chain message is the wrong size",
                PortTailLatency => "port tail/latency is missing",
                PortCount { .. } | PortChannels { .. } => "port description is truncated",
                RemapLayout => "remap layout is the wrong size",
                EmptyBody { .. } | PluginCommand => "unknown control message",
            },
            Self::PortCountOverflow { .. } => "port description is too large",
            Self::TrailingPortBytes { .. } => "port description has trailing bytes",
            Self::MissingMapping => "remap did not include a mapping",
            Self::UnknownMessageType(_) => "unknown control message",
            Self::UnknownPluginCommand(_) => "unknown plugin command",
        })
    }
}

impl std::error::Error for ControlDecodeError {}

pub(crate) fn read_fixed<const N: usize>(
    bytes: &[u8],
    field: ControlField,
) -> Result<[u8; N], ControlDecodeError> {
    bytes.try_into().map_err(|_| ControlDecodeError::WrongSize {
        field,
        expected: N,
        actual: bytes.len(),
    })
}

impl From<ControlDecodeError> for std::io::Error {
    fn from(error: ControlDecodeError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}
