//! Breaches of the control socket's transport rules: descriptors a peer
//! attached against the protocol, and frames this side refuses to send.
//! Timeouts, end of file and zero-byte writes are told apart by `ErrorKind`.

/// How a peer broke the rule of at most one descriptor per message, consumed
/// before the next arrives (SBX-A1 (2), ADR 0288). The read's bytes are
/// dropped, every descriptor it carried is closed, and each later receive on
/// the socket returns the same cause, so the message is never accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorRuleError {
    /// The kernel truncated the ancillary data. `received` is how many whole
    /// descriptors the read still delivered; all were closed.
    ControlTruncated { received: usize },
    /// One read carried `count` descriptors, more than one.
    ExtraDescriptors { count: usize },
    /// A descriptor arrived while an earlier one still waited for its message.
    UnconsumedDescriptor,
}

impl std::fmt::Display for DescriptorRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ControlTruncated { .. } => "control data was truncated",
            Self::ExtraDescriptors { .. } => {
                "a control message carried more than one file descriptor"
            }
            Self::UnconsumedDescriptor => {
                "a second file descriptor arrived before the first was consumed"
            }
        })
    }
}

impl std::error::Error for DescriptorRuleError {}

impl From<DescriptorRuleError> for std::io::Error {
    fn from(error: DescriptorRuleError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}

/// A frame this side refused to send. No byte of it was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlSendError {
    /// The frame's length field (type, request id and body) is above what
    /// every receiver accepts. `message_type` is the wire type.
    TooLong {
        message_type: u32,
        length: usize,
        maximum: usize,
    },
}

impl std::fmt::Display for ControlSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLong { .. } => "control message is longer than 64 MiB",
        })
    }
}

impl std::error::Error for ControlSendError {}

impl From<ControlSendError> for std::io::Error {
    fn from(error: ControlSendError) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, error)
    }
}
