//! Local transport failures. Wire replies are formatted only at their boundary.

use std::fmt;
use std::io;

use echode_history::PackageError;

/// The operation whose original I/O failure is retained by [`ApiError::Io`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiIoOperation {
    BindSocket,
    SetSocketPermissions,
    InspectSocketPath,
    ProbeSocket,
    RemoveStaleSocket,
    PeerCredentials,
    Accept,
    SetReadTimeout,
    SetWriteTimeout,
    CloneStream,
    ReadRequest,
    WriteReply,
    ReadMcpMessage,
    WriteMcpReply,
    FlushMcpReply,
    ConnectSession,
    WriteSessionRequest,
}

/// Why the local API or MCP transport stopped. No variant requires parsing text.
#[derive(Debug)]
pub enum ApiError {
    Io {
        operation: ApiIoOperation,
        error: io::Error,
    },
    Package(PackageError),
    SessionJson(serde_json::Error),
    ConnectionFromAnotherUser,
    SessionFromAnotherUser,
    SocketPathSymlink,
    SocketPathRegularFile,
    SocketPathNotSocket,
    SocketInUse,
    RequestLineTooLong,
    RequestLineIncomplete,
    McpMessageTooLong,
    McpMessageIncomplete,
    SessionRequestTooLong,
    SessionClosed,
    InvalidSessionResponse,
    UnsupportedSessionProtocol,
}

impl ApiError {
    pub(crate) fn io(operation: ApiIoOperation, error: io::Error) -> Self {
        Self::Io { operation, error }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, error } => {
                if *operation == ApiIoOperation::WriteReply {
                    f.write_str("could not write API reply: ")?;
                }
                write!(f, "{error}")
            }
            Self::Package(error) => write!(f, "{error}"),
            Self::SessionJson(error) => write!(f, "{error}"),
            Self::ConnectionFromAnotherUser => f.write_str("connection is from another user"),
            Self::SessionFromAnotherUser => f.write_str("session belongs to another user"),
            Self::SocketPathSymlink => f.write_str("socket path is a symbolic link"),
            Self::SocketPathRegularFile => f.write_str("socket path is a regular file"),
            Self::SocketPathNotSocket => f.write_str("socket path is not a socket"),
            Self::SocketInUse => f.write_str("socket is already in use"),
            Self::RequestLineTooLong => f.write_str("request line is too long"),
            Self::RequestLineIncomplete => f.write_str("request line ended without a newline"),
            Self::McpMessageTooLong => {
                f.write_str("MCP message exceeds the single-message size limit")
            }
            Self::McpMessageIncomplete => f.write_str("MCP message ended without a newline"),
            Self::SessionRequestTooLong => {
                f.write_str("session request exceeds the single-message size limit")
            }
            Self::SessionClosed => f.write_str("session closed without a response"),
            Self::InvalidSessionResponse => f.write_str("invalid session response"),
            Self::UnsupportedSessionProtocol => f.write_str("unsupported Echode session protocol"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Package(error) => Some(error),
            Self::SessionJson(error) => Some(error),
            _ => None,
        }
    }
}
