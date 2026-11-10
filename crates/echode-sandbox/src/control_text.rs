//! UTF-8 validation for text carried by the control socket, never audio data.

use std::io::{self, ErrorKind};
use std::str::Utf8Error;

/// A control text field failed UTF-8 validation. Offsets in the native error
/// are relative to that field, not to the enclosing frame. Invalid bytes are
/// not retained in the diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlTextError {
    WindowTitle(Utf8Error),
    Parameter(Utf8Error),
}

impl std::fmt::Display for ControlTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WindowTitle(_) => "window title is not text",
            Self::Parameter(_) => "parameter text is not UTF-8",
        })
    }
}

impl std::error::Error for ControlTextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::WindowTitle(error) | Self::Parameter(error) => error,
        })
    }
}

pub(crate) fn read_text(
    bytes: &[u8],
    cause: fn(Utf8Error) -> ControlTextError,
) -> io::Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|error| io::Error::new(ErrorKind::InvalidData, cause(error)))
}
