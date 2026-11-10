//! Payload kinds for diagnosing an internal helper main-thread reply mismatch.

/// The reply payload, not a socket message or a request number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MainReplyKind {
    State = 0,
    Done = 1,
    Parameters = 2,
    Values = 3,
    Parsed = 4,
    Window = 5,
}

impl MainReplyKind {
    pub(crate) fn decode(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::State,
            1 => Self::Done,
            2 => Self::Parameters,
            3 => Self::Values,
            4 => Self::Parsed,
            5 => Self::Window,
            _ => return None,
        })
    }

    pub(crate) fn mismatch_diagnostic(self) -> &'static str {
        match self {
            Self::State | Self::Done | Self::Window => "unexpected main-thread reply",
            Self::Parameters => "unexpected main-thread reply to parameter list",
            Self::Values => "unexpected main-thread reply to parameter values",
            Self::Parsed => "unexpected main-thread reply to parameter text",
        }
    }
}
