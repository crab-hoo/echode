//! Structural failures inside parameter, relay and window-report payloads,
//! and the reader that names the field it was reading when the bytes ran out.

use crate::control_text::{ControlTextError, read_text};
use std::io;

/// The parameter payload being read, named after what it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamPayload {
    /// Parameter descriptions with their values and text.
    Parameters,
    /// Requested value changes.
    Changes,
    /// Values read back after a change.
    AppliedValues,
    /// Typed text to convert for one parameter.
    TextRequest,
    /// How the plugin read typed text.
    ParsedText,
    /// Changes the plugin made on its own.
    Relay,
}

/// A field of a parameter payload. Sizes count only this field's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamField {
    /// The number of parameters or values in the payload's list.
    Count,
    Id,
    Flags,
    Min,
    Max,
    Default,
    Value,
    NameLength,
    Name,
    ModuleLength,
    Module,
    TextLength,
    Text,
    /// The outcome byte of a parsed-text reply.
    ParsedKind,
    RelayFlags,
    EventCount,
    EventId,
    EventKind,
}

/// One flag byte of a window report, in wire order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFlag {
    Created,
    Visible,
    Key,
}

/// Why a parameter, relay or window-report payload is invalid. Raw payload
/// bytes are not retained. Text keeps [`ControlTextError`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadDecodeError {
    /// `field` needs `expected` bytes and only `actual` remain. `entry` is the
    /// zero-based list entry being read, or `None` outside a list.
    Truncated {
        payload: ParamPayload,
        field: ParamField,
        entry: Option<usize>,
        expected: usize,
        actual: usize,
    },
    TrailingBytes {
        payload: ParamPayload,
        actual: usize,
    },
    /// A requested change that is NaN or infinite, as its IEEE-754 bits.
    NonFiniteChange {
        entry: usize,
        id: u32,
        bits: u64,
    },
    /// A converted text value that is NaN or infinite, as its IEEE-754 bits.
    NonFiniteParsedValue {
        bits: u64,
    },
    UnknownParsedKind(u8),
    WindowReportSize {
        expected: usize,
        actual: usize,
    },
    InvalidWindowFlag {
        flag: WindowFlag,
        value: u8,
    },
    /// The whole flag byte, including the known bits.
    UnknownRelayFlags(u8),
    UnknownEventKind {
        entry: usize,
        id: u32,
        kind: u8,
    },
}

impl std::fmt::Display for PayloadDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Truncated { .. } => "parameter message is truncated",
            Self::TrailingBytes { .. } => "parameter message has trailing bytes",
            Self::NonFiniteChange { .. } | Self::NonFiniteParsedValue { .. } => {
                "parameter value is not finite"
            }
            Self::UnknownParsedKind(_) => "unknown parameter text reply",
            Self::WindowReportSize { .. } => "window report is the wrong size",
            Self::InvalidWindowFlag { .. } => "window report flag is not 0 or 1",
            Self::UnknownRelayFlags(_) => "relay flags are unknown",
            Self::UnknownEventKind { .. } => "relay event kind is unknown",
        })
    }
}

impl std::error::Error for PayloadDecodeError {}

impl From<PayloadDecodeError> for io::Error {
    fn from(error: PayloadDecodeError) -> Self {
        Self::new(io::ErrorKind::InvalidData, error)
    }
}

/// Reads one payload front to back. Every list goes through [`Cursor::list`],
/// so the entry an error names and the bound on reserved memory have one rule.
pub(crate) struct Cursor<'a> {
    payload: ParamPayload,
    body: &'a [u8],
    at: usize,
    entry: Option<usize>,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(payload: ParamPayload, body: &'a [u8]) -> Self {
        Self {
            payload,
            body,
            at: 0,
            entry: None,
        }
    }

    fn unread(&self) -> &'a [u8] {
        self.body.get(self.at..).unwrap_or_default()
    }

    fn truncated(&self, field: ParamField, expected: usize) -> PayloadDecodeError {
        PayloadDecodeError::Truncated {
            payload: self.payload,
            field,
            entry: self.entry,
            expected,
            actual: self.unread().len(),
        }
    }

    fn take(&mut self, field: ParamField, len: usize) -> Result<&'a [u8], PayloadDecodeError> {
        let Some((bytes, _)) = self.unread().split_at_checked(len) else {
            return Err(self.truncated(field, len));
        };
        self.at += len;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self, field: ParamField) -> Result<[u8; N], PayloadDecodeError> {
        let Some((bytes, _)) = self.unread().split_first_chunk::<N>() else {
            return Err(self.truncated(field, N));
        };
        self.at += N;
        Ok(*bytes)
    }

    pub(crate) fn u8(&mut self, field: ParamField) -> Result<u8, PayloadDecodeError> {
        let [byte] = self.array(field)?;
        Ok(byte)
    }

    pub(crate) fn u32(&mut self, field: ParamField) -> Result<u32, PayloadDecodeError> {
        Ok(u32::from_le_bytes(self.array(field)?))
    }

    pub(crate) fn f64(&mut self, field: ParamField) -> Result<f64, PayloadDecodeError> {
        Ok(f64::from_bits(u64::from_le_bytes(self.array(field)?)))
    }

    /// A length-prefixed text. `length` and `text` name its two fields.
    pub(crate) fn text(&mut self, length: ParamField, text: ParamField) -> io::Result<String> {
        let len = self.u32(length)? as usize;
        read_text(self.take(text, len)?, ControlTextError::Parameter)
    }

    /// Every unread byte as text, for a payload that ends with unprefixed text.
    pub(crate) fn rest_text(&mut self) -> io::Result<String> {
        let bytes = self.unread();
        self.at = self.body.len();
        read_text(bytes, ControlTextError::Parameter)
    }

    /// A counted list. `read` gets the zero-based entry it is reading.
    /// Each entry needs at least `minimum` bytes, so a corrupt count cannot
    /// reserve more than the message already holds.
    pub(crate) fn list<T>(
        &mut self,
        count: ParamField,
        minimum: usize,
        mut read: impl FnMut(&mut Self, usize) -> io::Result<T>,
    ) -> io::Result<Vec<T>> {
        self.entry = None;
        let count = self.u32(count)? as usize;
        let room = self.unread().len().checked_div(minimum).unwrap_or(0);
        let mut items = Vec::with_capacity(count.min(room));
        for entry in 0..count {
            self.entry = Some(entry);
            items.push(read(self, entry)?);
        }
        self.entry = None;
        Ok(items)
    }

    pub(crate) fn finish(&self) -> Result<(), PayloadDecodeError> {
        match self.unread().len() {
            0 => Ok(()),
            actual => Err(PayloadDecodeError::TrailingBytes {
                payload: self.payload,
                actual,
            }),
        }
    }
}
