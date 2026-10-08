//! Product package: `project.json`, one file per track, and `journal.jsonl`.
//!
//! The same project always produces the same bytes. Each note is one line.
//! The journal's first line names the snapshot it applies to. Recovery replays
//! it only when that snapshot is still the latest autosave.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use echode_actions::{Action, decode, encode, replay};
use echode_model::{
    Clip, ClipKind, Id, Meter, Note, Project, Tempo, TempoMap, TempoPoint, Tick, Track,
};
use gix::objs::Tree;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};
use serde::Deserialize;

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
}

pub fn write_package(directory: &Path, project: &Project) -> Result<(), PackageError> {
    write_files(directory, &render(project))
}

pub fn read_package(directory: &Path) -> Result<Project, PackageError> {
    let project_path = directory.join("project.json");
    reject_symlink(&project_path)?;
    let project_json = read_to_string(&project_path)?;
    let header = parse_header(&project_json)?;
    let tracks_dir = directory.join("tracks");
    if !header.track_ids.is_empty() {
        reject_symlink(&tracks_dir)?;
    }
    let mut tracks = Vec::with_capacity(header.track_ids.len());
    for id in &header.track_ids {
        // HIS-8: the id was checked in parse_header, so this name stays inside tracks/.
        let filename = format!("{id}.json");
        let path = tracks_dir.join(&filename);
        reject_symlink(&path)?;
        let json = read_to_string(&path)?;
        tracks.push((filename, json));
    }
    assemble(header, tracks)
}

/// Replaces `journal.jsonl`. `groups` are the edits since `base`.
pub fn write_journal(
    directory: &Path,
    base: JournalBase,
    groups: &[Vec<Action>],
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
    let mut project_json = format!(
        "{{\n  \"schema\": 1,\n  \"meter\": {{\"numerator\": {}, \"denominator\": {}}},\n  \"tempo\": [\n",
        meter.numerator(),
        meter.denominator()
    );
    let points = project.tempo().points();
    for (index, point) in points.iter().enumerate() {
        let comma = if index + 1 == points.len() { "" } else { "," };
        let _ = writeln!(
            project_json,
            "    {{\"at\": {}, \"numerator\": {}, \"denominator\": {}}}{comma}",
            point.at.get(),
            point.tempo.numerator(),
            point.tempo.denominator()
        );
    }
    project_json.push_str("  ],\n  \"tracks\": [\n");
    let mut tracks = Vec::with_capacity(project.tracks().len());
    for (index, track) in project.tracks().iter().enumerate() {
        let comma = if index + 1 == project.tracks().len() {
            ""
        } else {
            ","
        };
        let id = track.id.to_string();
        let _ = writeln!(project_json, "    \"{id}\"{comma}");
        tracks.push((format!("{id}.json"), render_track(track)));
    }
    project_json.push_str("  ]\n}\n");
    PackageText {
        project_json,
        tracks,
    }
}

pub(crate) fn write_files(directory: &Path, package: &PackageText) -> Result<(), PackageError> {
    fs::create_dir_all(directory.join("tracks")).map_err(|error| io_error(directory, error))?;
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
    if header.track_ids.is_empty() {
        return assemble(header, Vec::new());
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
    assemble(header, tracks)
}

pub(crate) fn commit_tree(
    repo: &Repository,
    project_blob: ObjectId,
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
    let parent = match track.parent {
        Some(id) => format!("\"{id}\""),
        None => "null".to_string(),
    };
    let mut out = format!(
        "{{\n  \"schema\": 1,\n  \"id\": \"{}\",\n  \"name\": {},\n  \"parent\": {parent},\n  \"clips\": [\n",
        track.id,
        json_string(&track.name)
    );
    for (clip_index, clip) in track.clips.iter().enumerate() {
        let comma = if clip_index + 1 == track.clips.len() {
            ""
        } else {
            ","
        };
        let kind = match clip.kind {
            ClipKind::Auto => "auto",
            ClipKind::Fixed => "fixed",
        };
        let _ = write!(
            out,
            "    {{\n      \"id\": \"{}\",\n      \"start\": {},\n      \"length\": {},\n      \"kind\": \"{kind}\",\n      \"notes\": [\n",
            clip.id,
            clip.start.get(),
            clip.length.get()
        );
        for (note_index, note) in clip.notes.iter().enumerate() {
            let note_comma = if note_index + 1 == clip.notes.len() {
                ""
            } else {
                ","
            };
            let _ = writeln!(
                out,
                "        {{\"id\": \"{}\", \"start\": {}, \"length\": {}, \"pitch\": {}, \"velocity\": {}}}{note_comma}",
                note.id,
                note.start.get(),
                note.length.get(),
                note.pitch,
                note.velocity
            );
        }
        let _ = writeln!(out, "      ]\n    }}{comma}");
    }
    out.push_str("  ]\n}\n");
    out
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(other));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

struct Header {
    meter: Meter,
    tempo: TempoMap,
    track_ids: Vec<String>,
}

fn parse_header(json: &str) -> Result<Header, PackageError> {
    let file: ProjectFile = serde_json::from_str(json)
        .map_err(|error| PackageError::new(format!("project.json: {error}")))?;
    if file.schema != 1 {
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
        track_ids: canonical_track_ids(file.tracks)?,
    })
}

fn assemble(header: Header, tracks: Vec<(String, String)>) -> Result<Project, PackageError> {
    let mut parsed = Vec::with_capacity(tracks.len());
    if tracks.len() != header.track_ids.len() {
        return Err(PackageError::new(
            "track list does not match the track files",
        ));
    }
    for (id, (filename, json)) in header.track_ids.iter().zip(tracks) {
        if filename != format!("{id}.json") {
            return Err(PackageError::new(format!(
                "tracks/{filename} is out of order"
            )));
        }
        parsed.push(parse_track(&json, id)?);
    }
    Project::try_assemble(header.meter, header.tempo, parsed)
        .map_err(|error| PackageError::new(error.to_string()))
}

fn parse_track(json: &str, expected_id: &str) -> Result<Track, PackageError> {
    let file: TrackFile = serde_json::from_str(json)
        .map_err(|error| PackageError::new(format!("track json: {error}")))?;
    if file.schema != 1 {
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
        let kind = match clip.kind.as_str() {
            "auto" => ClipKind::Auto,
            "fixed" => ClipKind::Fixed,
            other => {
                return Err(PackageError::new(format!("unknown clip kind {other}")));
            }
        };
        let mut notes = Vec::with_capacity(clip.notes.len());
        for note in clip.notes {
            notes.push(Note {
                id: parse_id(&note.id)?,
                start: Tick::new(note.start),
                length: Tick::new(note.length),
                pitch: note.pitch,
                velocity: note.velocity,
            });
        }
        clips.push(Clip {
            id: parse_id(&clip.id)?,
            start: Tick::new(clip.start),
            length: Tick::new(clip.length),
            kind,
            notes,
        });
    }
    Ok(Track {
        id,
        name: file.name,
        parent,
        clips,
    })
}

fn parse_id(text: &str) -> Result<Id, PackageError> {
    Id::parse(text).ok_or_else(|| PackageError::new(format!("invalid id {text}")))
}

struct StoredJournal {
    base: JournalBase,
    groups: Vec<Vec<Action>>,
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

fn replay_on(project: &Project, groups: &[Vec<Action>]) -> Result<Project, PackageError> {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    schema: u32,
    meter: MeterFile,
    tempo: Vec<TempoFile>,
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
    clips: Vec<ClipFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipFile {
    id: String,
    start: i64,
    length: i64,
    kind: String,
    notes: Vec<NoteFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteFile {
    id: String,
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseLine {
    base: String,
}
