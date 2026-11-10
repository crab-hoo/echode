//! Typed document command and transaction failures.

use std::fmt;

use echode_actions::{ActionError, ParameterError};

use crate::PackageError;

/// Why the document thread rejected a command.
#[derive(Debug)]
pub enum HostError {
    Action(ActionError),
    Package(PackageError),
    /// One element of a transaction failed. Its cause selects the JSON-RPC code.
    Element {
        index: usize,
        error: ElementError,
    },
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Action(error) => write!(f, "{error}"),
            Self::Package(error) => write!(f, "{error}"),
            Self::Element { error, .. } => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Action(error) => Some(error),
            Self::Package(error) => Some(error),
            Self::Element { error, .. } => Some(error),
        }
    }
}

impl HostError {
    pub(super) fn durability_unknown(&self) -> bool {
        matches!(self, Self::Package(error) if error.is_durability_unknown())
    }
}

#[derive(Debug)]
pub enum ElementError {
    Reference(ReferenceError),
    Parameters(ParameterError),
    Package(PackageError),
    Action(ActionError),
}

impl ElementError {
    pub fn is_parameters(&self) -> bool {
        matches!(self, Self::Reference(_) | Self::Parameters(_))
    }
}

impl fmt::Display for ElementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reference(error) => write!(f, "{error}"),
            Self::Parameters(error) => write!(f, "{error}"),
            Self::Package(error) => write!(f, "{error}"),
            Self::Action(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ElementError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reference(error) => Some(error),
            Self::Parameters(error) => Some(error),
            Self::Package(error) => Some(error),
            Self::Action(error) => Some(error),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    NotIndex,
    MissingField,
    OutOfRange,
    FieldNotFound,
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotIndex => "action reference is not an index",
            Self::MissingField => "action reference is missing a field",
            Self::OutOfRange => "action reference is out of range",
            Self::FieldNotFound => "action reference field is missing",
        })
    }
}

impl std::error::Error for ReferenceError {}
