//! Shared schema migration and validation for working files and Git revisions.

use std::path::Path;

use echode_model::{
    Content, ContentMetadata, Id, Note, Placement, Project, ProjectMetadata, TempoMap, TempoPoint,
    Track, TrackMetadata,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{MigrationError, PackageError, PackageKind, PackageValidationError as Invalid};

/// Current package schema. Older numbers are raised by `SCHEMA_STEPS`.
pub(super) const SCHEMA: u32 = 6;

type SchemaStep = fn(&mut Value) -> Result<(), MigrationError>;

/// Index 0 raises schema 1 to 2. Missing fields retain their historical defaults.
const SCHEMA_STEPS: &[SchemaStep] = &[
    |value| add_device_field(value, "plugin", Value::Null),
    audio_reference,
    |value| add_device_field(value, "bypassed", Value::Bool(false)),
    |value| add_device_field(value, "offline", Value::Bool(false)),
    |value| add_device_field(value, "pinned", Value::Array(Vec::new())),
];

fn add_device_field(value: &mut Value, field: &str, default: Value) -> Result<(), MigrationError> {
    let Some(devices) = value.get_mut("devices") else {
        return Ok(());
    };
    let devices = devices
        .as_array_mut()
        .ok_or(MigrationError::DevicesNotArray)?;
    for device in devices {
        let device = device
            .as_object_mut()
            .ok_or(MigrationError::DeviceNotObject)?;
        device.entry(field).or_insert_with(|| default.clone());
    }
    Ok(())
}

/// Schema 2's object ids were never files. Schema 3's hash objects are left alone.
fn audio_reference(value: &mut Value) -> Result<(), MigrationError> {
    if let Some(audio) = value.get_mut("audio")
        && audio.is_string()
    {
        *audio = Value::Null;
    }
    Ok(())
}

fn migrate(mut value: Value, kind: PackageKind) -> Result<Value, Invalid> {
    let mut schema = schema_number(&value, kind)?;
    if schema > SCHEMA {
        return Err(Invalid::UnsupportedSchema {
            kind,
            schema: u64::from(schema),
        });
    }
    while schema < SCHEMA {
        let index = schema.checked_sub(1).ok_or(Invalid::UnsupportedSchema {
            kind,
            schema: u64::from(schema),
        })?;
        let step = usize::try_from(index)
            .ok()
            .and_then(|index| SCHEMA_STEPS.get(index))
            .ok_or(Invalid::UnsupportedSchema {
                kind,
                schema: u64::from(schema),
            })?;
        step(&mut value).map_err(|error| Invalid::Migration {
            kind,
            schema,
            error,
        })?;
        schema += 1;
        // schema_number established an object and every step preserves that shape.
        let object = value.as_object_mut().ok_or(Invalid::NotObject(kind))?;
        object.insert("schema".to_owned(), Value::from(schema));
    }
    Ok(value)
}

fn schema_number(value: &Value, kind: PackageKind) -> Result<u32, Invalid> {
    let object = value.as_object().ok_or(Invalid::NotObject(kind))?;
    let schema = object.get("schema").ok_or(Invalid::MissingSchema(kind))?;
    let number = schema
        .as_number()
        .filter(|number| number.is_u64())
        .and_then(|number| number.as_u64())
        .ok_or(Invalid::NonIntegerSchema(kind))?;
    u32::try_from(number).map_err(|_| Invalid::UnsupportedSchema {
        kind,
        schema: number,
    })
}

fn parse_schema<T: DeserializeOwned>(json: &str, kind: PackageKind) -> Result<T, PackageError> {
    let value: Value = serde_json::from_str(json).map_err(|error| Invalid::Json { kind, error })?;
    let value = migrate(value, kind)?;
    serde_json::from_value(value).map_err(|error| Invalid::Json { kind, error }.into())
}

pub(super) struct Header {
    metadata: ProjectMetadata,
    tempo: TempoMap,
    pub(super) content_ids: Vec<String>,
    pub(super) track_ids: Vec<String>,
}

pub(super) fn parse_header(json: &str) -> Result<Header, PackageError> {
    let file: ProjectFile = parse_schema(json, PackageKind::Project)?;
    let metadata =
        serde_json::from_value(Value::Object(file.metadata)).map_err(|error| Invalid::Json {
            kind: PackageKind::Project,
            error,
        })?;
    let tempo = TempoMap::try_from_points(file.tempo).ok_or(Invalid::InvalidTempoMap)?;
    Ok(Header {
        metadata,
        tempo,
        content_ids: canonical_ids(file.contents)?,
        track_ids: canonical_ids(file.tracks)?,
    })
}

pub(super) fn assemble(
    header: Header,
    contents: Vec<(String, String)>,
    tracks: Vec<(String, String)>,
    package: &Path,
) -> Result<Project, PackageError> {
    if tracks.len() != header.track_ids.len() {
        return Err(Invalid::FileCountMismatch(PackageKind::Track).into());
    }
    if contents.len() != header.content_ids.len() {
        return Err(Invalid::FileCountMismatch(PackageKind::Content).into());
    }
    let mut parsed_contents = Vec::with_capacity(contents.len());
    for (id, (filename, json)) in header.content_ids.iter().zip(contents) {
        check_filename(id, &filename, PackageKind::Content)?;
        parsed_contents.push(parse_content(&json, id)?);
    }
    let mut parsed = Vec::with_capacity(tracks.len());
    for (id, (filename, json)) in header.track_ids.iter().zip(tracks) {
        check_filename(id, &filename, PackageKind::Track)?;
        parsed.push(parse_track(&json, id, package)?);
    }
    header
        .metadata
        .try_assemble(header.tempo, parsed_contents, parsed)
        .map_err(|error| Invalid::Model(error).into())
}

fn check_filename(id: &str, filename: &str, kind: PackageKind) -> Result<(), Invalid> {
    if filename != format!("{id}.json") {
        return Err(Invalid::FileOutOfOrder {
            kind,
            filename: filename.to_owned(),
        });
    }
    Ok(())
}

fn parse_track(json: &str, expected_id: &str, package: &Path) -> Result<Track, PackageError> {
    let file: TrackFile = parse_schema(json, PackageKind::Track)?;
    // Keep unknown-field rejection in the scalar type, not flatten + deny_unknown_fields.
    let metadata: TrackMetadata =
        serde_json::from_value(Value::Object(file.metadata)).map_err(|error| Invalid::Json {
            kind: PackageKind::Track,
            error,
        })?;
    if file.id != expected_id {
        return Err(Invalid::IdMismatch {
            kind: PackageKind::Track,
            id: file.id,
        }
        .into());
    }
    let id = parse_id(&file.id)?;
    let clips = file.clips;
    let devices = file
        .devices
        .into_iter()
        .map(|device| {
            let kind = device.kind;
            match (kind, device.state.as_deref()) {
                (_, None) => {}
                (echode_model::DeviceKind::Mixer, Some(_)) => {
                    return Err(Invalid::MixerState.into());
                }
                (echode_model::DeviceKind::Clap, Some(text)) => {
                    if !crate::objects::is_content_hash(text) {
                        return Err(Invalid::InvalidStateHash.into());
                    }
                    crate::objects::read_object(package, text)?;
                }
            }
            match (kind, device.plugin.as_deref()) {
                (echode_model::DeviceKind::Mixer, None) => {}
                (echode_model::DeviceKind::Clap, Some(plugin))
                    if echode_model::valid_plugin_id(plugin) && device.parameters.is_empty() => {}
                _ => return Err(Invalid::DevicePluginMismatch(device.id).into()),
            }
            Ok(device)
        })
        .collect::<Result<Vec<_>, PackageError>>()?;
    Ok(metadata.into_track(id, clips, devices))
}

fn parse_content(json: &str, expected_id: &str) -> Result<Content, PackageError> {
    let file: ContentFile = parse_schema(json, PackageKind::Content)?;
    let metadata: ContentMetadata =
        serde_json::from_value(Value::Object(file.metadata)).map_err(|error| Invalid::Json {
            kind: PackageKind::Content,
            error,
        })?;
    if let Some(audio) = &metadata.audio {
        audio.check().map_err(Invalid::Model)?;
    }
    if file.id != expected_id {
        return Err(Invalid::IdMismatch {
            kind: PackageKind::Content,
            id: file.id,
        }
        .into());
    }
    if file.notes.iter().any(|note| note.articulation.is_some()) {
        return Err(Invalid::Articulation.into());
    }
    if let Some(expression) = file.expressions.first() {
        return Err(Invalid::Expression {
            note: expression.note.clone(),
            kind: expression.kind.clone(),
            points: expression.points.len(),
            first_at: expression.points.first().map(|point| point.at).unwrap_or(0),
            first_value: expression
                .points
                .first()
                .map(|point| point.value)
                .unwrap_or(0),
        }
        .into());
    }
    Ok(metadata.into_content(parse_id(&file.id)?, file.notes, Vec::new()))
}

fn parse_id(text: &str) -> Result<Id, Invalid> {
    Id::parse(text).ok_or_else(|| Invalid::InvalidId(text.to_owned()))
}

fn canonical_ids(ids: Vec<String>) -> Result<Vec<String>, Invalid> {
    ids.into_iter()
        .map(|id| {
            Id::parse(&id)
                .map(|id| id.to_string())
                .ok_or(Invalid::InvalidListedId(id))
        })
        .collect()
}

#[derive(Deserialize)]
struct ProjectFile {
    // parse_schema consumes and validates the version before deserialization.
    #[serde(rename = "schema")]
    _schema: u32,
    #[serde(flatten)]
    metadata: serde_json::Map<String, Value>,
    tempo: Vec<TempoPoint>,
    #[serde(default)]
    contents: Vec<String>,
    tracks: Vec<String>,
}

#[derive(Deserialize)]
struct TrackFile {
    #[serde(rename = "schema")]
    _schema: u32,
    id: String,
    #[serde(flatten)]
    metadata: serde_json::Map<String, Value>,
    devices: Vec<echode_model::Device>,
    clips: Vec<Placement>,
}

#[derive(Deserialize)]
struct ContentFile {
    #[serde(rename = "schema")]
    _schema: u32,
    id: String,
    #[serde(flatten)]
    metadata: serde_json::Map<String, Value>,
    notes: Vec<Note>,
    expressions: Vec<ExpressionFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpressionFile {
    note: String,
    kind: String,
    points: Vec<ExpressionPointFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpressionPointFile {
    at: i64,
    value: i32,
}

#[cfg(test)]
#[path = "document_parse_tests.rs"]
mod tests;
