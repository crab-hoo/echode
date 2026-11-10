//! Structural failures in the helper's refusal payload, before a refusal exists.

/// A fixed-size field within a refusal. Sizes count only this field's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalField {
    MainCall,
    ParameterId,
    MainReplies,
    /// Bytes following this plugin failure's kind (a port, or no payload).
    PluginPayload {
        kind: u8,
    },
}

/// Why a refusal payload is invalid. Raw payload bytes are not retained.
/// Launch payloads keep their own [`crate::LaunchDecodeError`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalDecodeError {
    MissingKind,
    UnknownKind(u8),
    TrailingBytes {
        kind: u8,
        actual: usize,
    },
    WrongSize {
        field: RefusalField,
        expected: usize,
        actual: usize,
    },
    UnknownExpectedReply(u8),
    UnknownActualReply(u8),
    MissingPluginKind,
    UnknownPluginKind(u8),
    InvalidPortSide {
        kind: u8,
        side: u8,
    },
}

impl std::fmt::Display for RefusalDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use RefusalField::*;
        f.write_str(match self {
            Self::MissingKind => "refusal has no kind",
            Self::UnknownKind(_) => "refusal has an unknown kind",
            Self::TrailingBytes { .. } => "refusal has trailing bytes",
            Self::WrongSize { field, .. } => match field {
                MainCall => "refusal call number is the wrong size",
                ParameterId => "refusal parameter id is the wrong size",
                MainReplies => "refusal reply kinds are the wrong size",
                PluginPayload { .. } => "plugin failure is malformed",
            },
            Self::UnknownExpectedReply(_) => "refusal expected reply kind is unknown",
            Self::UnknownActualReply(_) => "refusal actual reply kind is unknown",
            Self::MissingPluginKind | Self::UnknownPluginKind(_) | Self::InvalidPortSide { .. } => {
                "plugin failure is malformed"
            }
        })
    }
}

impl std::error::Error for RefusalDecodeError {}
