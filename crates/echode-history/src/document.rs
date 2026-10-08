//! Product package: `project.json`, one file per track, and `journal.jsonl`.
//!
//! The same project always produces the same bytes. Each note is one line.
//! A file older than the current schema is raised one version at a time on a
//! JSON value, then parsed with unknown fields rejected. Schema 1 is current,
//! so that list is empty (ADR 0028). The journal's first line names the
//! snapshot it applies to. Recovery applies each later line's changeset, or
//! replays its commands when the line has none, and only when that snapshot
//! is still the latest autosave.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use echode_actions::{JournalEntry, decode, encode, replay};
use echode_model::{ClipKind, Id, Meter, Note, Project, Tempo, TempoMap, TempoPoint, Tick, Track};
use gix::objs::Tree;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Autosave commits land here until a project has more than one alternate.
pub const AUTOSAVE_REF: &str = "refs/echode/autosave/main";

/// Where a journal's actions start from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalBase {
    /// The package files, before the first autosave commit.
    Files,
    /// An autosave commit.
    Commit(ObjectId),
}

/// A package or journal could not be read or written.
#[derive(Debug)]
pub struct PackageError {
    message: String,
}

impl PackageError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PackageError {}

pub(crate) struct PackageText {
    pub project_json: String,
    pub tracks: Vec<(String, String)>,
    pub contents: Vec<(String, String)>,
}

pub fn write_package(directory: &Path, project: &Project) -> Result<(), PackageError> {
    write_files(directory, &render(project))
}

pub fn read_package(directory: &Path) -> Result<Project, PackageError> {
    let project_path = directory.join("project.json");
    reject_symlink(&project_path)?;
    let project_json = read_to_string(&project_path)?;
    let header = parse_header(&project_json)?;
    let contents = read_named_dir(directory, "contents", &header.content_ids)?;
    let tracks = read_named_dir(directory, "tracks", &header.track_ids)?;
    assemble(header, contents, tracks)
}

fn read_named_dir(
    directory: &Path,
    folder: &str,
    ids: &[String],
) -> Result<Vec<(String, String)>, PackageError> {
    let dir = directory.join(folder);
    if !ids.is_empty() {
        reject_symlink(&dir)?;
    }
    let mut files = Vec::with_capacity(ids.len());
    for id in ids {
        // HIS-8: the id was checked before this name is joined.
        let filename = format!("{id}.json");
        let path = dir.join(&filename);
        reject_symlink(&path)?;
        let json = read_to_string(&path)?;
        files.push((filename, json));
    }
    Ok(files)
}

/// Replaces `journal.jsonl`. `groups` are the edits since `base`.
pub fn write_journal(
    directory: &Path,
    base: JournalBase,
    groups: &[JournalEntry],
) -> Result<(), PackageError> {
    let token = match &base {
        JournalBase::Files => "files".to_string(),
        JournalBase::Commit(id) => id.to_string(),
    };
    let mut text = format!("{{\"base\":\"{token}\"}}\n");
    text.push_str(&encode(groups));
    let destination = directory.join("journal.jsonl");
    let temporary = directory.join("journal.jsonl.tmp");
    fs::write(&temporary, text).map_err(|error| io_error(&temporary, error))?;
    fs::rename(&temporary, &destination).map_err(|error| io_error(&destination, error))?;
    Ok(())
}

/// Loads the latest durable snapshot, then replays a journal that still names it.
pub fn recover(directory: &Path) -> Result<Project, PackageError> {
    let tip = autosave_tip(directory);
    let Some(journal) = read_journal(directory)? else {
        return match tip {
            Some(id) => read_commit_at(directory, id),
            None => read_package(directory),
        };
    };
    match journal.base {
        JournalBase::Files => {
            if let Some(id) = tip {
                return read_commit_at(directory, id);
            }
            replay_on(&read_package(directory)?, &journal.groups)
        }
        JournalBase::Commit(id) => {
            if let Some(tip) = tip
                && tip != id
            {
                return read_commit_at(directory, tip);
            }
            replay_on(&read_commit_at(directory, id)?, &journal.groups)
        }
    }
}

pub(crate) fn render(project: &Project) -> PackageText {
    let meter = project.meter();
    let mut project_json = String::from("{\n");
    push_field(&mut project_json, "schema", &to_json(&SCHEMA));
    push_field(
        &mut project_json,
        "meter",
        &to_json(&MeterLine {
            numerator: meter.numerator(),
            denominator: meter.denominator(),
        }),
    );
    project_json.push_str("  \"tempo\":[\n");
    push_json_lines(
        &mut project_json,
        "    ",
        project.tempo().points().iter().map(|point| {
            to_json(&TempoLine {
                at: point.at.get(),
                numerator: point.tempo.numerator(),
                denominator: point.tempo.denominator(),
            })
        }),
    );
    project_json.push_str("  ],\n  \"contents\":[\n");
    let mut contents = Vec::with_capacity(project.contents().len());
    let mut content_ids = Vec::with_capacity(project.contents().len());
    for content in project.contents() {
        let id = content.id.to_string();
        contents.push((format!("{id}.json"), render_content(content)));
        content_ids.push(to_json(&id));
    }
    push_json_lines(&mut project_json, "    ", content_ids.into_iter());
    project_json.push_str("  ],\n  \"tracks\":[\n");
    let mut tracks = Vec::with_capacity(project.tracks().len());
    let mut track_ids = Vec::with_capacity(project.tracks().len());
    for track in project.tracks() {
        let id = track.id.to_string();
        tracks.push((format!("{id}.json"), render_track(track)));
        track_ids.push(to_json(&id));
    }
    push_json_lines(&mut project_json, "    ", track_ids.into_iter());
    project_json.push_str("  ]\n}\n");
    PackageText {
        project_json,
        tracks,
        contents,
    }
}

pub(crate) fn write_files(directory: &Path, package: &PackageText) -> Result<(), PackageError> {
    fs::create_dir_all(directory.join("tracks")).map_err(|error| io_error(directory, error))?;
    fs::create_dir_all(directory.join("contents")).map_err(|error| io_error(directory, error))?;
    let project_path = directory.join("project.json");
    fs::write(&project_path, &package.project_json)
        .map_err(|error| io_error(&project_path, error))?;
    let mut keep = HashSet::with_capacity(package.tracks.len());
    for (filename, json) in &package.tracks {
        let path = directory.join("tracks").join(filename);
        fs::write(&path, json).map_err(|error| io_error(&path, error))?;
        keep.insert(filename.clone());
    }
    let tracks_dir = directory.join("tracks");
    let entries = fs::read_dir(&tracks_dir).map_err(|error| io_error(&tracks_dir, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(&tracks_dir, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".json") && !keep.contains(name.as_ref()) {
            fs::remove_file(entry.path()).map_err(|error| io_error(&entry.path(), error))?;
        }
    }
    let mut keep_contents = HashSet::with_capacity(package.contents.len());
    for (filename, json) in &package.contents {
        let path = directory.join("contents").join(filename);
        fs::write(&path, json).map_err(|error| io_error(&path, error))?;
        keep_contents.insert(filename.clone());
    }
    let contents_dir = directory.join("contents");
    if contents_dir.exists() {
        let entries =
            fs::read_dir(&contents_dir).map_err(|error| io_error(&contents_dir, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&contents_dir, error))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json") && !keep_contents.contains(name.as_ref()) {
                fs::remove_file(entry.path()).map_err(|error| io_error(&entry.path(), error))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn read_commit(repo: &Repository, revision: ObjectId) -> Result<Project, PackageError> {
    let commit = repo
        .find_commit(revision)
        .map_err(|error| PackageError::new(error.to_string()))?;
    let root = commit
        .tree()
        .map_err(|error| PackageError::new(error.to_string()))?;
    let project_json = blob_text(&root, "project.json")?;
    let header = parse_header(&project_json)?;
    let contents = read_tree_files(&root, "contents", &header.content_ids)?;
    if header.track_ids.is_empty() {
        return assemble(header, contents, Vec::new());
    }
    let tracks_entry = root
        .lookup_entry_by_path("tracks")
        .map_err(|error| PackageError::new(error.to_string()))?
        .ok_or_else(|| PackageError::new("tracks directory is missing from the revision"))?;
    let tracks_tree = tracks_entry
        .object()
        .map_err(|error| PackageError::new(error.to_string()))?
        .try_into_tree()
        .map_err(|error| PackageError::new(format!("{error:?}")))?;
    let mut by_name = std::collections::HashMap::with_capacity(header.track_ids.len());
    for entry in tracks_tree.iter() {
        let entry = entry.map_err(|error| PackageError::new(error.to_string()))?;
        let filename = entry.filename().to_string();
        let blob = entry
            .object()
            .map_err(|error| PackageError::new(error.to_string()))?
            .try_into_blob()
            .map_err(|error| PackageError::new(format!("{error:?}")))?;
        let json = std::str::from_utf8(&blob.data)
            .map_err(|error| PackageError::new(error.to_string()))?
            .to_string();
        by_name.insert(filename, json);
    }
    let mut tracks = Vec::with_capacity(header.track_ids.len());
    for id in &header.track_ids {
        let filename = format!("{id}.json");
        let json = by_name
            .remove(&filename)
            .ok_or_else(|| PackageError::new(format!("tracks/{filename} is missing")))?;
        tracks.push((filename, json));
    }
    assemble(header, contents, tracks)
}

fn read_tree_files(
    root: &gix::Tree<'_>,
    folder: &str,
    ids: &[String],
) -> Result<Vec<(String, String)>, PackageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let entry = root
        .lookup_entry_by_path(folder)
        .map_err(|error| PackageError::new(error.to_string()))?
        .ok_or_else(|| {
            PackageError::new(format!("{folder} directory is missing from the revision"))
        })?;
    let tree = entry
        .object()
        .map_err(|error| PackageError::new(error.to_string()))?
        .try_into_tree()
        .map_err(|error| PackageError::new(format!("{error:?}")))?;
    let mut by_name = std::collections::HashMap::with_capacity(ids.len());
    for entry in tree.iter() {
        let entry = entry.map_err(|error| PackageError::new(error.to_string()))?;
        let filename = entry.filename().to_string();
        let blob = entry
            .object()
            .map_err(|error| PackageError::new(error.to_string()))?
            .try_into_blob()
            .map_err(|error| PackageError::new(format!("{error:?}")))?;
        let json = std::str::from_utf8(&blob.data)
            .map_err(|error| PackageError::new(error.to_string()))?
            .to_string();
        by_name.insert(filename, json);
    }
    let mut files = Vec::with_capacity(ids.len());
    for id in ids {
        let filename = format!("{id}.json");
        let json = by_name
            .remove(&filename)
            .ok_or_else(|| PackageError::new(format!("{folder}/{filename} is missing")))?;
        files.push((filename, json));
    }
    Ok(files)
}

pub(crate) fn commit_tree(
    repo: &Repository,
    project_blob: ObjectId,
    content_blobs: &[(String, ObjectId)],
    track_blobs: &[(String, ObjectId)],
    parents: &[ObjectId],
) -> Result<ObjectId, PackageError> {
    let mut track_entries = Vec::with_capacity(track_blobs.len());
    for (filename, blob) in track_blobs {
        track_entries.push(Entry {
            mode: EntryKind::Blob.into(),
            filename: gix::bstr::BString::from(filename.clone()),
            oid: *blob,
        });
    }
    track_entries.sort();
    let mut root_entries = vec![Entry {
        mode: EntryKind::Blob.into(),
        filename: gix::bstr::BString::from("project.json"),
        oid: project_blob,
    }];
    if !content_blobs.is_empty() {
        let mut content_entries = Vec::with_capacity(content_blobs.len());
        for (filename, blob) in content_blobs {
            content_entries.push(Entry {
                mode: EntryKind::Blob.into(),
                filename: gix::bstr::BString::from(filename.clone()),
                oid: *blob,
            });
        }
        content_entries.sort();
        let contents_tree = repo
            .write_object(Tree {
                entries: content_entries,
            })
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach();
        root_entries.push(Entry {
            mode: EntryKind::Tree.into(),
            filename: gix::bstr::BString::from("contents"),
            oid: contents_tree,
        });
    }
    if !track_entries.is_empty() {
        let tracks_tree = repo
            .write_object(Tree {
                entries: track_entries,
            })
            .map_err(|error| PackageError::new(error.to_string()))?
            .detach();
        root_entries.push(Entry {
            mode: EntryKind::Tree.into(),
            filename: gix::bstr::BString::from("tracks"),
            oid: tracks_tree,
        });
    }
    root_entries.sort();
    let root = repo
        .write_object(Tree {
            entries: root_entries,
        })
        .map_err(|error| PackageError::new(error.to_string()))?
        .detach();
    let author = gix::actor::Signature {
        name: "Echode".into(),
        email: "history@echode.local".into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let commit = repo
        .commit_as(
            author.to_ref(&mut committer_time),
            author.to_ref(&mut author_time),
            AUTOSAVE_REF,
            "autosave",
            root,
            parents.iter().copied(),
        )
        .map_err(|error| PackageError::new(error.to_string()))?;
    Ok(commit.detach())
}

fn render_track(track: &Track) -> String {
    let mut out = String::from("{\n");
    push_field(&mut out, "schema", &to_json(&SCHEMA));
    push_field(&mut out, "id", &to_json(&track.id.to_string()));
    push_field(&mut out, "name", &to_json(&track.name));
    push_field(
        &mut out,
        "parent",
        &to_json(&track.parent.map(|id| id.to_string())),
    );
    push_field(&mut out, "order", &to_json(&track.order));
    out.push_str("  \"devices\":[\n");
    for (index, device) in track.devices.iter().enumerate() {
        let comma = if index + 1 == track.devices.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(
            out,
            "    {{\"id\":{},\"kind\":{},\"state\":{},\"parameters\":[",
            to_json(&device.id.to_string()),
            to_json(device.kind.as_str()),
            to_json(&None::<String>)
        );
        push_json_lines(
            &mut out,
            "      ",
            device.parameters.iter().map(|parameter| {
                to_json(&ParameterLine {
                    id: parameter.id.to_string(),
                    key: parameter.key.as_str(),
                    min: parameter.min,
                    max: parameter.max,
                    default: parameter.default,
                    value: parameter.value,
                })
            }),
        );
        let _ = writeln!(out, "    ]}}{comma}");
    }
    out.push_str("  ],\n  \"clips\":[\n");
    push_json_lines(
        &mut out,
        "    ",
        track.clips.iter().map(|clip| {
            let kind = match clip.kind {
                ClipKind::Auto => "auto",
                ClipKind::Fixed => "fixed",
            };
            to_json(&PlacementLine {
                id: clip.id.to_string(),
                content: clip.content.to_string(),
                lane: "timeline",
                start: clip.start.get(),
                length: clip.length.get(),
                offset: clip.offset.get(),
                kind,
                gain: clip.gain,
                fade_in: clip.fade_in,
                fade_out: clip.fade_out,
            })
        }),
    );
    out.push_str("  ]\n}\n");
    out
}

fn render_content(content: &echode_model::Content) -> String {
    let takes: Vec<String> = content.takes.iter().map(|id| id.to_string()).collect();
    let mut out = String::from("{\n");
    push_field(&mut out, "schema", &to_json(&SCHEMA));
    push_field(&mut out, "id", &to_json(&content.id.to_string()));
    push_field(
        &mut out,
        "audio",
        &to_json(&content.audio.map(|id| id.to_string())),
    );
    push_field(&mut out, "takes", &to_json(&takes));
    out.push_str("  \"notes\":[\n");
    push_json_lines(
        &mut out,
        "    ",
        content.notes.iter().map(|note| {
            to_json(&NoteLine {
                id: note.id.to_string(),
                start: note.start.get(),
                length: note.length.get(),
                pitch: note.pitch,
                velocity: note.velocity,
                release: note.release,
                articulation: note.articulation.map(|id| id.to_string()),
            })
        }),
    );
    out.push_str("  ],\n  \"expressions\":");
    out.push_str(&to_json(&Vec::<serde_json::Value>::new()));
    out.push_str("\n}\n");
    out
}

fn format_gain(gain: f32) -> String {
    if gain == 0.0 {
        return "0".to_string();
    }
    if gain == 1.0 {
        return "1".to_string();
    }
    let mut text = format!("{gain:.9}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

/// Current package schema. Older numbers are raised by [`SCHEMA_STEPS`].
const SCHEMA: u32 = 1;

/// Raises one schema number. Index 0 raises schema 1 to schema 2.
type SchemaStep = fn(&mut serde_json::Value) -> Result<(), String>;

/// Empty while schema 1 is current. A later schema keeps the schema-1 fixture
/// and appends one function here.
const SCHEMA_STEPS: &[SchemaStep] = &[];

fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str("  \"");
    out.push_str(key);
    out.push_str("\":");
    out.push_str(value);
    out.push_str(",\n");
}

fn push_json_lines(out: &mut String, indent: &str, values: impl Iterator<Item = String>) {
    let values: Vec<String> = values.collect();
    for (index, value) in values.iter().enumerate() {
        out.push_str(indent);
        out.push_str(value);
        if index + 1 != values.len() {
            out.push(',');
        }
        out.push('\n');
    }
}

fn to_json(value: &(impl Serialize + ?Sized)) -> String {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, PackageFormatter);
    value
        .serialize(&mut serializer)
        .expect("package values are finite JSON");
    String::from_utf8(buffer).expect("package JSON is UTF-8")
}

/// serde_json writes strings and structure. `f32` keeps the ADR 0022 spelling,
/// so 0 stays `0` and 1 stays `1`.
struct PackageFormatter;

impl serde_json::ser::Formatter for PackageFormatter {
    fn write_f32<W>(&mut self, writer: &mut W, value: f32) -> std::io::Result<()>
    where
        W: ?Sized + std::io::Write,
    {
        writer.write_all(format_gain(value).as_bytes())
    }
}

fn migrate(mut value: serde_json::Value, kind: &str) -> Result<serde_json::Value, PackageError> {
    let mut schema = schema_number(&value, kind)?;
    if schema > SCHEMA {
        return Err(PackageError::new(format!(
            "unsupported {kind} schema {schema}"
        )));
    }
    while schema < SCHEMA {
        let index = schema
            .checked_sub(1)
            .ok_or_else(|| PackageError::new(format!("unsupported {kind} schema {schema}")))?;
        let index = usize::try_from(index)
            .map_err(|_| PackageError::new(format!("unsupported {kind} schema {schema}")))?;
        let step = SCHEMA_STEPS
            .get(index)
            .copied()
            .ok_or_else(|| PackageError::new(format!("unsupported {kind} schema {schema}")))?;
        step(&mut value)
            .map_err(|message| PackageError::new(format!("{kind} schema {schema}: {message}")))?;
        schema += 1;
        let object = value
            .as_object_mut()
            .ok_or_else(|| PackageError::new(format!("{kind} json is not an object")))?;
        object.insert("schema".to_owned(), serde_json::Value::from(schema));
    }
    Ok(value)
}

fn schema_number(value: &serde_json::Value, kind: &str) -> Result<u32, PackageError> {
    let Some(object) = value.as_object() else {
        return Err(PackageError::new(format!("{kind} json is not an object")));
    };
    let Some(schema) = object.get("schema") else {
        return Err(PackageError::new(format!("{kind} schema is missing")));
    };
    let Some(number) = schema.as_number().filter(|number| number.is_u64()) else {
        return Err(PackageError::new(format!(
            "{kind} schema is not an integer"
        )));
    };
    let Some(schema) = number.as_u64() else {
        return Err(PackageError::new(format!(
            "{kind} schema is not an integer"
        )));
    };
    u32::try_from(schema)
        .map_err(|_| PackageError::new(format!("unsupported {kind} schema {schema}")))
}

fn parse_schema<T: DeserializeOwned>(
    json: &str,
    file: &str,
    kind: &str,
) -> Result<T, PackageError> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| PackageError::new(format!("{file}: {error}")))?;
    let value = migrate(value, kind)?;
    serde_json::from_value(value).map_err(|error| PackageError::new(format!("{file}: {error}")))
}

struct Header {
    meter: Meter,
    tempo: TempoMap,
    content_ids: Vec<String>,
    track_ids: Vec<String>,
}

fn parse_header(json: &str) -> Result<Header, PackageError> {
    let file: ProjectFile = parse_schema(json, "project.json", "project")?;
    if file.schema != SCHEMA {
        return Err(PackageError::new(format!(
            "unsupported project schema {}",
            file.schema
        )));
    }
    let meter = Meter::try_new(file.meter.numerator, file.meter.denominator)
        .ok_or_else(|| PackageError::new("meter does not divide into ticks"))?;
    let mut points = Vec::with_capacity(file.tempo.len());
    for point in file.tempo {
        let tempo = Tempo::rational(point.numerator, point.denominator)
            .ok_or_else(|| PackageError::new("tempo is outside the allowed range"))?;
        points.push(TempoPoint {
            at: Tick::new(point.at),
            tempo,
        });
    }
    let tempo = TempoMap::try_from_points(points)
        .ok_or_else(|| PackageError::new("tempo map is empty or has two points at one tick"))?;
    Ok(Header {
        meter,
        tempo,
        content_ids: canonical_track_ids(file.contents)?,
        track_ids: canonical_track_ids(file.tracks)?,
    })
}

fn assemble(
    header: Header,
    contents: Vec<(String, String)>,
    tracks: Vec<(String, String)>,
) -> Result<Project, PackageError> {
    let mut parsed = Vec::with_capacity(tracks.len());
    if tracks.len() != header.track_ids.len() {
        return Err(PackageError::new(
            "track list does not match the track files",
        ));
    }
    if contents.len() != header.content_ids.len() {
        return Err(PackageError::new(
            "content list does not match the content files",
        ));
    }
    let mut parsed_contents = Vec::with_capacity(contents.len());
    for (id, (filename, json)) in header.content_ids.iter().zip(contents) {
        if filename != format!("{id}.json") {
            return Err(PackageError::new(format!(
                "contents/{filename} is out of order"
            )));
        }
        parsed_contents.push(parse_content(&json, id)?);
    }
    for (id, (filename, json)) in header.track_ids.iter().zip(tracks) {
        if filename != format!("{id}.json") {
            return Err(PackageError::new(format!(
                "tracks/{filename} is out of order"
            )));
        }
        parsed.push(parse_track(&json, id)?);
    }
    Project::try_assemble(header.meter, header.tempo, parsed_contents, parsed)
        .map_err(|error| PackageError::new(error.to_string()))
}

fn parse_track(json: &str, expected_id: &str) -> Result<Track, PackageError> {
    let file: TrackFile = parse_schema(json, "track json", "track")?;
    if file.schema != SCHEMA {
        return Err(PackageError::new(format!(
            "unsupported track schema {}",
            file.schema
        )));
    }
    if file.id != expected_id {
        return Err(PackageError::new(format!(
            "track file {} does not contain that id",
            file.id
        )));
    }
    let id = parse_id(&file.id)?;
    let parent = match file.parent.as_deref() {
        None => None,
        Some(text) => Some(parse_id(text)?),
    };
    let mut clips = Vec::with_capacity(file.clips.len());
    for clip in file.clips {
        if clip.lane != "timeline" {
            return Err(PackageError::new(format!("unknown lane {}", clip.lane)));
        }
        let kind = match clip.kind.as_str() {
            "auto" => ClipKind::Auto,
            "fixed" => ClipKind::Fixed,
            other => {
                return Err(PackageError::new(format!("unknown clip kind {other}")));
            }
        };
        let gain = clip.gain;
        if !gain.is_finite() || gain < 0.0 {
            return Err(PackageError::new("gain must be finite and at least zero"));
        }
        clips.push(echode_model::Placement {
            id: parse_id(&clip.id)?,
            content: parse_id(&clip.content)?,
            lane: echode_model::Lane::Timeline,
            start: Tick::new(clip.start),
            length: Tick::new(clip.length),
            offset: Tick::new(clip.offset),
            kind,
            gain,
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
        });
    }
    Ok(Track {
        id,
        name: file.name,
        parent,
        order: file.order,
        devices: file
            .devices
            .into_iter()
            .map(|device| {
                match device.state.as_deref() {
                    None => {}
                    Some(text) if crate::objects::is_content_hash(text) => {
                        return Err(PackageError::new(format!(
                            "plugin state is not accepted yet ({text})"
                        )));
                    }
                    Some(_) => {
                        return Err(PackageError::new(
                            "state hash must be 64 lowercase hex digits",
                        ));
                    }
                }
                let kind = echode_model::DeviceKind::parse(&device.kind).ok_or_else(|| {
                    PackageError::new(format!("unknown device kind {}", device.kind))
                })?;
                let mut parameters = Vec::with_capacity(device.parameters.len());
                for parameter in device.parameters {
                    let key = echode_model::ParamKey::parse(&parameter.key).ok_or_else(|| {
                        PackageError::new(format!("unknown parameter {}", parameter.key))
                    })?;
                    parameters.push(echode_model::Parameter {
                        id: parse_id(&parameter.id)?,
                        key,
                        min: parameter.min,
                        max: parameter.max,
                        default: parameter.default,
                        value: parameter.value,
                    });
                }
                Ok(echode_model::Device {
                    id: parse_id(&device.id)?,
                    kind,
                    parameters,
                })
            })
            .collect::<Result<Vec<_>, PackageError>>()?,
        clips,
    })
}

fn parse_content(json: &str, expected_id: &str) -> Result<echode_model::Content, PackageError> {
    let file: ContentFile = parse_schema(json, "content json", "content")?;
    if file.schema != SCHEMA {
        return Err(PackageError::new(format!(
            "unsupported content schema {}",
            file.schema
        )));
    }
    if file.id != expected_id {
        return Err(PackageError::new(format!(
            "content file {} does not contain that id",
            file.id
        )));
    }
    let mut notes = Vec::with_capacity(file.notes.len());
    for note in file.notes {
        if note.articulation.is_some() {
            return Err(PackageError::new("articulation is not accepted yet"));
        }
        notes.push(Note {
            id: parse_id(&note.id)?,
            start: Tick::new(note.start),
            length: Tick::new(note.length),
            pitch: note.pitch,
            velocity: note.velocity,
            release: note.release,
            articulation: None,
        });
    }
    if let Some(expression) = file.expressions.first() {
        return Err(PackageError::new(format!(
            "expressions are not accepted yet ({} {} has {} points, first at {} value {})",
            expression.note,
            expression.kind,
            expression.points.len(),
            expression.points.first().map(|point| point.at).unwrap_or(0),
            expression
                .points
                .first()
                .map(|point| point.value)
                .unwrap_or(0)
        )));
    }
    let mut takes = Vec::with_capacity(file.takes.len());
    for take in file.takes {
        takes.push(parse_id(&take)?);
    }
    Ok(echode_model::Content {
        id: parse_id(&file.id)?,
        audio: file.audio.as_deref().map(parse_id).transpose()?,
        takes,
        notes,
        expressions: Vec::new(),
    })
}

fn parse_id(text: &str) -> Result<Id, PackageError> {
    Id::parse(text).ok_or_else(|| PackageError::new(format!("invalid id {text}")))
}

struct StoredJournal {
    base: JournalBase,
    groups: Vec<JournalEntry>,
}

fn read_journal(directory: &Path) -> Result<Option<StoredJournal>, PackageError> {
    let path = directory.join("journal.jsonl");
    if !path.exists() {
        return Ok(None);
    }
    let text = read_to_string(&path)?;
    if text.is_empty() {
        return Ok(None);
    }
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| PackageError::new("journal is missing its base"))?;
    let base_line: BaseLine = serde_json::from_str(header)
        .map_err(|error| PackageError::new(format!("journal base: {error}")))?;
    let base = match base_line.base.as_str() {
        "files" => JournalBase::Files,
        hex => JournalBase::Commit(
            ObjectId::from_hex(hex.as_bytes())
                .map_err(|error| PackageError::new(format!("journal base: {error}")))?,
        ),
    };
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    let groups = if body.is_empty() {
        Vec::new()
    } else {
        decode(&body).map_err(|error| PackageError::new(error.to_string()))?
    };
    Ok(Some(StoredJournal { base, groups }))
}

fn replay_on(project: &Project, groups: &[JournalEntry]) -> Result<Project, PackageError> {
    replay(project, groups).map_err(|error| PackageError::new(error.to_string()))
}

fn read_commit_at(directory: &Path, revision: ObjectId) -> Result<Project, PackageError> {
    let repo = gix::open(directory).map_err(|error| PackageError::new(error.to_string()))?;
    read_commit(&repo, revision)
}

pub(crate) fn autosave_tip(directory: &Path) -> Option<ObjectId> {
    let repo = gix::open(directory).ok()?;
    let reference = repo.try_find_reference(AUTOSAVE_REF).ok()??;
    Some(reference.id().detach())
}

fn blob_text(root: &gix::Tree<'_>, path: &str) -> Result<String, PackageError> {
    let entry = root
        .lookup_entry_by_path(path)
        .map_err(|error| PackageError::new(error.to_string()))?
        .ok_or_else(|| PackageError::new(format!("{path} is missing from the revision")))?;
    let blob = entry
        .object()
        .map_err(|error| PackageError::new(error.to_string()))?
        .try_into_blob()
        .map_err(|error| PackageError::new(format!("{error:?}")))?;
    std::str::from_utf8(&blob.data)
        .map_err(|error| PackageError::new(error.to_string()))
        .map(ToOwned::to_owned)
}

fn read_to_string(path: &Path) -> Result<String, PackageError> {
    reject_symlink(path)?;
    fs::read_to_string(path).map_err(|error| io_error(path, error))
}

/// HIS-8: a package entry must be a real file. Following a link could leave the package.
fn reject_symlink(path: &Path) -> Result<(), PackageError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(PackageError::new(format!(
            "{} is a symbolic link",
            path.display()
        )));
    }
    Ok(())
}

fn canonical_track_ids(ids: Vec<String>) -> Result<Vec<String>, PackageError> {
    ids.into_iter()
        .map(|id| {
            Id::parse(&id)
                .map(|id| id.to_string())
                .ok_or_else(|| PackageError::new(format!("invalid track id {id}")))
        })
        .collect()
}

fn io_error(path: &Path, error: std::io::Error) -> PackageError {
    PackageError::new(format!("{}: {error}", path.display()))
}

#[derive(Serialize)]
struct MeterLine {
    numerator: u32,
    denominator: u32,
}

#[derive(Serialize)]
struct TempoLine {
    at: i64,
    numerator: u32,
    denominator: u32,
}

#[derive(Serialize)]
struct ParameterLine<'a> {
    id: String,
    key: &'a str,
    min: f32,
    max: f32,
    default: f32,
    value: f32,
}

#[derive(Serialize)]
struct PlacementLine<'a> {
    id: String,
    content: String,
    lane: &'a str,
    start: i64,
    length: i64,
    offset: i64,
    kind: &'a str,
    gain: f32,
    fade_in: i64,
    fade_out: i64,
}

#[derive(Serialize)]
struct NoteLine {
    id: String,
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u16,
    release: u16,
    articulation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    schema: u32,
    meter: MeterFile,
    tempo: Vec<TempoFile>,
    #[serde(default)]
    contents: Vec<String>,
    tracks: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeterFile {
    numerator: u32,
    denominator: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TempoFile {
    at: i64,
    numerator: u32,
    denominator: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackFile {
    schema: u32,
    id: String,
    name: String,
    #[serde(default)]
    parent: Option<String>,
    order: String,
    devices: Vec<DeviceFile>,
    clips: Vec<ClipFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceFile {
    id: String,
    kind: String,
    state: Option<String>,
    parameters: Vec<ParameterFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParameterFile {
    id: String,
    key: String,
    min: f32,
    max: f32,
    default: f32,
    value: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipFile {
    id: String,
    content: String,
    lane: String,
    start: i64,
    length: i64,
    offset: i64,
    kind: String,
    gain: f32,
    fade_in: i64,
    fade_out: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentFile {
    schema: u32,
    id: String,
    audio: Option<String>,
    takes: Vec<String>,
    notes: Vec<NoteFile>,
    expressions: Vec<ExpressionFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteFile {
    id: String,
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u16,
    release: u16,
    articulation: Option<String>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseLine {
    base: String,
}

#[cfg(test)]
mod tests {
    use super::{format_gain, to_json};
    use serde::Serialize;

    #[test]
    fn gain_keeps_the_short_spelling() {
        assert_eq!(format_gain(0.0), "0");
        assert_eq!(format_gain(-0.0), "0");
        assert_eq!(format_gain(1.0), "1");
        assert_eq!(format_gain(4.0), "4");
        assert_eq!(format_gain(-1.0), "-1");
        assert_eq!(format_gain(0.5), "0.5");
    }

    #[test]
    fn a_gain_serializes_as_that_spelling() {
        #[derive(Serialize)]
        struct Row {
            gain: f32,
        }
        assert_eq!(to_json(&Row { gain: 1.0 }), "{\"gain\":1}");
        assert_eq!(to_json(&Row { gain: 0.0 }), "{\"gain\":0}");
        assert_eq!(to_json(&Row { gain: -1.0 }), "{\"gain\":-1}");
        assert_eq!(to_json(&"pi\"ano"), "\"pi\\\"ano\"");
    }
}
