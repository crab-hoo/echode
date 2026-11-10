//! Typed schema and package validation failures; display text is not a protocol.

use std::fmt;

use echode_model::{Id, ModelError};

/// The saved object being decoded or migrated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageKind {
    Project,
    Track,
    Content,
}

impl PackageKind {
    pub(crate) fn json_label(self) -> &'static str {
        match self {
            Self::Project => "project.json",
            Self::Track => "track json",
            Self::Content => "content json",
        }
    }
}

impl fmt::Display for PackageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Project => "project",
            Self::Track => "track",
            Self::Content => "content",
        })
    }
}

/// A legacy device field could not be added to malformed JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationError {
    DevicesNotArray,
    DeviceNotObject,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DevicesNotArray => "devices is not an array",
            Self::DeviceNotObject => "device is not an object",
        })
    }
}

impl std::error::Error for MigrationError {}

/// A saved package was rejected before publishing a project.
#[derive(Debug)]
pub enum PackageValidationError {
    Json {
        kind: PackageKind,
        error: serde_json::Error,
    },
    NotObject(PackageKind),
    MissingSchema(PackageKind),
    NonIntegerSchema(PackageKind),
    UnsupportedSchema {
        kind: PackageKind,
        schema: u64,
    },
    Migration {
        kind: PackageKind,
        schema: u32,
        error: MigrationError,
    },
    InvalidTempoMap,
    FileCountMismatch(PackageKind),
    FileOutOfOrder {
        kind: PackageKind,
        filename: String,
    },
    IdMismatch {
        kind: PackageKind,
        id: String,
    },
    InvalidId(String),
    /// Both track and content lists historically use the label "track id".
    InvalidListedId(String),
    MixerState,
    InvalidStateHash,
    DevicePluginMismatch(Id),
    Articulation,
    Expression {
        note: String,
        kind: String,
        points: usize,
        first_at: i64,
        first_value: i32,
    },
    Model(ModelError),
}

impl fmt::Display for PackageValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { kind, error } => write!(f, "{}: {error}", kind.json_label()),
            Self::NotObject(kind) => write!(f, "{kind} json is not an object"),
            Self::MissingSchema(kind) => write!(f, "{kind} schema is missing"),
            Self::NonIntegerSchema(kind) => write!(f, "{kind} schema is not an integer"),
            Self::UnsupportedSchema { kind, schema } => {
                write!(f, "unsupported {kind} schema {schema}")
            }
            Self::Migration {
                kind,
                schema,
                error,
            } => write!(f, "{kind} schema {schema}: {error}"),
            Self::InvalidTempoMap => {
                f.write_str("tempo map is empty or has two points at one tick")
            }
            Self::FileCountMismatch(kind) => {
                write!(f, "{kind} list does not match the {kind} files")
            }
            Self::FileOutOfOrder { kind, filename } => {
                write!(f, "{kind}s/{filename} is out of order")
            }
            Self::IdMismatch { kind, id } => write!(f, "{kind} file {id} does not contain that id"),
            Self::InvalidId(id) => write!(f, "invalid id {id}"),
            Self::InvalidListedId(id) => write!(f, "invalid track id {id}"),
            Self::MixerState => f.write_str("mixer state must be null"),
            Self::InvalidStateHash => f.write_str("state hash must be 64 lowercase hex digits"),
            Self::DevicePluginMismatch(id) => write!(f, "device {id} does not match its plugin id"),
            Self::Articulation => f.write_str("articulation is not accepted yet"),
            Self::Expression {
                note,
                kind,
                points,
                first_at,
                first_value,
            } => write!(
                f,
                "expressions are not accepted yet ({note} {kind} has {points} points, first at {first_at} value {first_value})"
            ),
            Self::Model(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PackageValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json { error, .. } => Some(error),
            Self::Migration { error, .. } => Some(error),
            Self::Model(error) => Some(error),
            _ => None,
        }
    }
}
