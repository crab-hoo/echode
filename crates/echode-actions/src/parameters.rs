//! Typed command decoding. Serde checks shape; model constructors check values.

use std::fmt;

use echode_model::{Id, Meter, NoteDraft, Tempo, Tick};
use serde::Deserialize;

/// Why JSON could not become a model command. No edit has been performed.
#[derive(Debug)]
pub enum ParameterError {
    Json(serde_json::Error),
    InvalidId { field: &'static str, value: String },
    InvalidMeter { numerator: u32, denominator: u32 },
    InvalidTempo { numerator: u32, denominator: u32 },
}

impl fmt::Display for ParameterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(_) => f.write_str("parameters do not match an action"),
            Self::InvalidId { value, .. } => write!(f, "invalid id {value}"),
            Self::InvalidMeter { .. } => f.write_str("meter does not divide into ticks"),
            Self::InvalidTempo { .. } => f.write_str("tempo is outside the allowed range"),
        }
    }
}

impl std::error::Error for ParameterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InvalidId { .. } | Self::InvalidMeter { .. } | Self::InvalidTempo { .. } => None,
        }
    }
}

/// The registry applies this conversion to every command field. Adding a new
/// field type requires choosing its wire shape and validation explicitly.
pub(crate) trait Parameter: Sized {
    type Wire: for<'de> Deserialize<'de>;

    fn decode(wire: Self::Wire, field: &'static str) -> Result<Self, ParameterError>;
}

macro_rules! identity {
    ($($ty:ty),+ $(,)?) => {$(
        impl Parameter for $ty {
            type Wire = Self;

            fn decode(wire: Self, _field: &'static str) -> Result<Self, ParameterError> {
                Ok(wire)
            }
        }
    )+};
}

identity!(String, bool, u8, u16, u32, u64, i64, f32, Tick, NoteDraft);

impl Parameter for Id {
    type Wire = String;

    fn decode(value: String, field: &'static str) -> Result<Self, ParameterError> {
        Self::parse(&value).ok_or(ParameterError::InvalidId { field, value })
    }
}

impl<T: Parameter> Parameter for Option<T> {
    type Wire = Option<T::Wire>;

    fn decode(wire: Self::Wire, field: &'static str) -> Result<Self, ParameterError> {
        wire.map(|value| T::decode(value, field)).transpose()
    }
}

#[derive(Deserialize)]
pub(crate) struct Fraction {
    numerator: u32,
    denominator: u32,
}

impl Parameter for Meter {
    type Wire = Fraction;

    fn decode(wire: Fraction, _field: &'static str) -> Result<Self, ParameterError> {
        Self::try_new(wire.numerator, wire.denominator).ok_or(ParameterError::InvalidMeter {
            numerator: wire.numerator,
            denominator: wire.denominator,
        })
    }
}

impl Parameter for Tempo {
    type Wire = Fraction;

    fn decode(wire: Fraction, _field: &'static str) -> Result<Self, ParameterError> {
        Self::rational(wire.numerator, wire.denominator).ok_or(ParameterError::InvalidTempo {
            numerator: wire.numerator,
            denominator: wire.denominator,
        })
    }
}
