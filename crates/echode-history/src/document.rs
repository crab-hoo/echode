//! Product package: `project.json`, one file per track, and `journal.jsonl`.
//!
//! The same project always produces the same bytes. Each note is one line.
//! A file older than the current schema is raised one version at a time on a
//! JSON value, then parsed with unknown fields rejected. Schema 6 is current.
//! The journal is an append-only log.
//! Recovery replays the lines that continue after the newest readable commit.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;

use crate::package_io::Directory;
use echode_model::{
    ContentMetadata, Id, Note, Placement, Project, ProjectMetadata, TempoMap, TempoPoint, Track,
    TrackMetadata,
};
use gix::objs::Tree;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[path = "document_device.rs"]
mod device_json;
#[path = "document_track.rs"]
mod track_json;

/// Autosave commits land here until a project has more than one alternate.
pub const AUTOSAVE_REF: &str = "refs/echode/autosave/main";

/// A package or journal could not be read or written.
#[derive(Debug)]
pub struct PackageError {
    message: String,
    durability_unknown: bool,
}

impl PackageError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            durability_unknown: false,
        }
    }

    pub(crate) fn durability_unknown() -> Self {
        Self {
            message: JOURNAL_DURABILITY_UNKNOWN.into(),
            durability_unknown: true,
        }
    }

    /// The journal line reached the file and a later sync or rollback did not
    /// finish. Later edits stop until the package is opened again (AUD-03).
    pub fn is_durability_unknown(&self) -> bool {
        self.durability_unknown
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
    let root = open_package(directory)?;
    let project_path = directory.join("project.json");
    let project_json = root
        .read_text("project.json")
        .map_err(|error| io_error(&project_path, error))?;
    let header = parse_header(&project_json)?;
    let contents = read_named_dir(&root, directory, "contents", &header.content_ids)?;
    let tracks = read_named_dir(&root, directory, "tracks", &header.track_ids)?;
    assemble(header, contents, tracks, directory)
}

fn read_named_dir(
    root: &Directory,
    directory: &Path,
    folder: &str,
    ids: &[String],
) -> Result<Vec<(String, String)>, PackageError> {
    let dir = directory.join(folder);
    let Some(owned) = root
        .optional_child(folder)
        .map_err(|error| io_error(&dir, error))?
    else {
        return if ids.is_empty() {
            Ok(Vec::new())
        } else {
            Err(PackageError::new(format!("{} is missing", dir.display())))
        };
    };
    let mut files = Vec::with_capacity(ids.len());
    for id in ids {
        // HIS-8: the id was checked before this name is joined.
        let filename = format!("{id}.json");
        let path = dir.join(&filename);
        let json = owned
            .read_text(&filename)
            .map_err(|error| io_error(&path, error))?;
        files.push((filename, json));
    }
    Ok(files)
}

/// The write reached the file and a later sync or rollback did not finish.
pub(crate) const JOURNAL_DURABILITY_UNKNOWN: &str = "journal durability is unknown";

/// Appends one synced journal line. The document thread is the only caller.
///
/// A torn tail is cut back to the last newline and synced before the new line
/// is added, so a crash during the previous append cannot glue two records together.
pub fn append_journal(
    directory: &Path,
    line: &echode_actions::JournalLine,
) -> Result<(), PackageError> {
    repair_journal(directory)?;
    let mut text = echode_actions::encode_line(line);
    text.push('\n');
    let path = directory.join("journal.jsonl");
    let mut file = open_package(directory)?
        .open_file("journal.jsonl", true, true)
        .map_err(|error| io_error(&path, error))?;
    let end = file
        .seek(SeekFrom::End(0))
        .map_err(|error| io_error(&path, error))?;
    let bytes = text.into_bytes();
    let fault = take_journal_fault(directory);
    let wrote = match fault {
        JournalFault::BeforeWrite => Err(PackageError::new("journal write failed")),
        JournalFault::Partial => {
            let torn = if bytes.is_empty() {
                &bytes[..]
            } else {
                &bytes[..1]
            };
            file.write_all(torn)
                .map_err(|error| io_error(&path, error))?;
            Err(PackageError::new("journal write failed"))
        }
        JournalFault::Sync | JournalFault::Uncertain => file
            .write_all(&bytes)
            .map_err(|error| io_error(&path, error))
            .and(Err(if fault == JournalFault::Uncertain {
                PackageError::durability_unknown()
            } else {
                PackageError::new("journal sync failed")
            })),
        JournalFault::None => file
            .write_all(&bytes)
            .map_err(|error| io_error(&path, error)),
    };
    if let Err(error) = wrote {
        if fault == JournalFault::Uncertain {
            return Err(error);
        }
        file.set_len(end).map_err(|error| io_error(&path, error))?;
        file.sync_all().map_err(|error| io_error(&path, error))?;
        return Err(error);
    }
    if let Err(error) = file.sync_all() {
        match file.set_len(end).and_then(|()| file.sync_all()) {
            Ok(()) => return Err(io_error(&path, error)),
            Err(_) => return Err(PackageError::durability_unknown()),
        }
    }
    Ok(())
}

/// Drops an incomplete trailing record and syncs that shorter file.
pub(crate) fn repair_journal(directory: &Path) -> Result<(), PackageError> {
    let root = open_package(directory)?;
    let path = directory.join("journal.jsonl");
    if !root
        .has_file("journal.jsonl")
        .map_err(|error| io_error(&path, error))?
    {
        return Ok(());
    }
    let mut file = root
        .open_file("journal.jsonl", true, false)
        .map_err(|error| io_error(&path, error))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|error| io_error(&path, error))?;
    let committed = committed_journal(&bytes);
    if committed.len() == bytes.len() {
        return Ok(());
    }
    file.set_len(committed.len() as u64)
        .map_err(|error| io_error(&path, error))?;
    file.sync_all().map_err(|error| io_error(&path, error))?;
    Ok(())
}

/// Bytes through the last newline. A trailing partial record is not committed.
fn committed_journal(bytes: &[u8]) -> &[u8] {
    match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => &bytes[..=index],
        None => &[],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum JournalFault {
    None,
    BeforeWrite,
    Partial,
    Sync,
    /// The bytes stay in the file. The caller must stop editing and recover.
    Uncertain,
}

fn take_journal_fault(directory: &Path) -> JournalFault {
    #[cfg(test)]
    {
        let path = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        let mut faults = JOURNAL_FAULTS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = faults.iter().position(|(candidate, _)| candidate == &path) {
            return faults.swap_remove(index).1;
        }
    }
    let _ = directory;
    JournalFault::None
}

#[cfg(test)]
static JOURNAL_FAULTS: Mutex<Vec<(PathBuf, JournalFault)>> = Mutex::new(Vec::new());

/// The next journal append for `directory` fails in the requested way.
#[cfg(test)]
pub(crate) fn fail_next_journal(directory: &Path, fault: JournalFault) {
    let path = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    JOURNAL_FAULTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((path, fault));
}

/// Drops lines at or below `through`. The kept suffix is synced, then renamed.
pub fn compact_journal(directory: &Path, through: u64) -> Result<(), PackageError> {
    let records = read_log(directory)?;
    let kept: Vec<_> = records
        .into_iter()
        .filter(|line| line.seq > through)
        .collect();
    let mut text = String::new();
    for line in &kept {
        text.push_str(&echode_actions::encode_line(line));
        text.push('\n');
    }
    let root = open_package(directory)?;
    root.write_atomic("journal.jsonl", text.as_bytes(), true)
        .map_err(|error| io_error(directory, error))?;
    root.sync().map_err(|error| io_error(directory, error))?;
    Ok(())
}

/// Marks a save that has started rewriting the worktree and has not committed.
pub fn mark_save_pending(directory: &Path) -> Result<(), PackageError> {
    let path = directory.join("save.pending");
    open_package(directory)?
        .write_atomic("save.pending", b"pending\n", true)
        .map_err(|error| io_error(&path, error))?;
    Ok(())
}

pub fn clear_save_pending(directory: &Path) -> Result<(), PackageError> {
    let path = directory.join("save.pending");
    open_package(directory)?
        .remove("save.pending")
        .map_err(|error| io_error(&path, error))?;
    Ok(())
}

pub(crate) fn save_pending(directory: &Path) -> bool {
    Directory::open(directory)
        .and_then(|root| root.has_file("save.pending"))
        .unwrap_or(true)
}

pub(crate) fn open_package(directory: &Path) -> Result<Directory, PackageError> {
    let root = Directory::open(directory).map_err(|error| io_error(directory, error))?;
    root.validate_layout()
        .map_err(|error| io_error(directory, error))?;
    Ok(root)
}

/// Why an opened document waits for the user's consent before any edit
/// (ADR 0183): recent edits may be missing from what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    /// The autosave tip could not be read, so an older commit was used.
    UnreadableTip,
    /// Journal lines continue after a missing one and were not replayed.
    JournalGap,
    /// A journal line names plugin state the store does not hold intact.
    /// It and the lines after it were not replayed (HIS-A1).
    MissingState,
}

/// The project recovery chose, the commit it was read from, and the last journal
/// line included. `base_revision` is empty when there was no commit.
pub(crate) struct Recovered {
    pub project: Project,
    pub base_revision: String,
    pub applied_seq: u64,
    pub tip_revision: Option<String>,
    pub reason: Option<RecoveryReason>,
}

/// Loads the newest readable commit, then replays the contiguous journal suffix.
/// Refuses recovery if any replayed transaction breaks the model invariants.
pub fn recover(directory: &Path) -> Result<Project, PackageError> {
    Ok(recover_labeled(directory)?.project)
}

pub(crate) fn recover_labeled(directory: &Path) -> Result<Recovered, PackageError> {
    // Validate the worktree even when recovery will read a commit instead of its JSON.
    open_package(directory)?;
    let records = read_log(directory)?;
    let pending = save_pending(directory);
    let tip = autosave_tip(directory);
    if let Some(id) = tip {
        let Some((commit, project, seq)) = first_readable_commit(directory, id)? else {
            return Err(PackageError::new("no readable commit"));
        };
        let replayed = replay_suffix(directory, project, seq, &records)?;
        let reason = if commit == id {
            replayed.stopped
        } else {
            Some(RecoveryReason::UnreadableTip)
        };
        return Ok(Recovered {
            project: replayed.project,
            base_revision: commit.to_string(),
            applied_seq: replayed.applied,
            tip_revision: Some(id.to_string()),
            reason,
        });
    }
    if pending {
        return Err(PackageError::new(
            "worktree save is unfinished and there is no commit",
        ));
    }
    let project = read_package(directory)?;
    let replayed = replay_suffix(directory, project, 0, &records)?;
    Ok(Recovered {
        project: replayed.project,
        base_revision: String::new(),
        applied_seq: replayed.applied,
        tip_revision: None,
        reason: replayed.stopped,
    })
}

pub(crate) fn render(project: &Project) -> PackageText {
    PackageText {
        project_json: render_project_only(project),
        tracks: project
            .tracks()
            .iter()
            .map(|track| (format!("{}.json", track.id), render_track(track)))
            .collect(),
        contents: project
            .contents()
            .iter()
            .map(|content| (format!("{}.json", content.id), render_content(content)))
            .collect(),
    }
}

fn render_project_only(project: &Project) -> String {
    #[derive(Serialize)]
    struct Header {
        schema: u32,
        #[serde(flatten)]
        metadata: ProjectMetadata,
    }
    let mut project_json = track_json::render_metadata(&Header {
        schema: SCHEMA,
        metadata: ProjectMetadata::from(project),
    });
    project_json.push_str("  \"tempo\":[\n");
    push_json_lines(
        &mut project_json,
        "    ",
        project.tempo().points().iter().map(to_json),
    );
    project_json.push_str("  ],\n  \"contents\":[\n");
    push_json_lines(
        &mut project_json,
        "    ",
        project
            .contents()
            .iter()
            .map(|content| to_json(&content.id.to_string())),
    );
    project_json.push_str("  ],\n  \"tracks\":[\n");
    push_json_lines(
        &mut project_json,
        "    ",
        project
            .tracks()
            .iter()
            .map(|track| to_json(&track.id.to_string())),
    );
    project_json.push_str("  ]\n}\n");
    project_json
}

pub(crate) fn write_files(directory: &Path, package: &PackageText) -> Result<(), PackageError> {
    Directory::create(directory).map_err(|error| io_error(directory, error))?;
    let root = open_package(directory)?;
    let tracks = root
        .child("tracks")
        .map_err(|error| io_error(directory, error))?;
    let contents = root
        .child("contents")
        .map_err(|error| io_error(directory, error))?;
    root.write_atomic("project.json", package.project_json.as_bytes(), false)
        .map_err(|error| io_error(directory, error))?;
    let mut keep = HashSet::with_capacity(package.tracks.len());
    for (filename, json) in &package.tracks {
        let path = directory.join("tracks").join(filename);
        tracks
            .write_atomic(filename, json.as_bytes(), false)
            .map_err(|error| io_error(&path, error))?;
        keep.insert(filename.clone());
    }
    delete_unlisted(&tracks, &directory.join("tracks"), &keep)?;
    let mut keep_contents = HashSet::with_capacity(package.contents.len());
    for (filename, json) in &package.contents {
        let path = directory.join("contents").join(filename);
        contents
            .write_atomic(filename, json.as_bytes(), false)
            .map_err(|error| io_error(&path, error))?;
        keep_contents.insert(filename.clone());
    }
    delete_unlisted(&contents, &directory.join("contents"), &keep_contents)?;
    Ok(())
}

/// Writes changed tracks and contents. `project.json` is written when its text
/// differs from `previous_project_json`.
///
/// Each written file is replaced by renaming a temporary file. `sweep` removes
/// JSON files the project no longer lists. Later saves pass the ids that left
/// the cache and skip the directory scan.
pub(crate) fn write_incremental(
    directory: &Path,
    project: &Project,
    plan: &FilePlan<'_>,
) -> Result<IncrementalText, PackageError> {
    let root = open_package(directory)?;
    let track_dir = root
        .child("tracks")
        .map_err(|error| io_error(directory, error))?;
    let content_dir = root
        .child("contents")
        .map_err(|error| io_error(directory, error))?;
    let project_json = render_project_only(project);
    if plan.previous_project_json != Some(project_json.as_str()) {
        root.write_atomic("project.json", project_json.as_bytes(), false)
            .map_err(|error| io_error(directory, error))?;
    }
    let mut tracks = Vec::new();
    let mut keep = HashSet::new();
    for track in project.tracks() {
        let id = track.id.to_string();
        let filename = format!("{id}.json");
        if plan.sweep {
            keep.insert(filename.clone());
        }
        if plan.unchanged_tracks.contains(&id) {
            continue;
        }
        let json = render_track(track);
        track_dir
            .write_atomic(&filename, json.as_bytes(), false)
            .map_err(|error| io_error(directory, error))?;
        tracks.push((filename, json));
    }
    if plan.sweep {
        delete_unlisted(&track_dir, &directory.join("tracks"), &keep)?;
    } else {
        remove_named(&track_dir, directory, "tracks", plan.removed_tracks)?;
    }
    let mut contents = Vec::new();
    let mut keep_contents = HashSet::new();
    for content in project.contents() {
        let id = content.id.to_string();
        let filename = format!("{id}.json");
        if plan.sweep {
            keep_contents.insert(filename.clone());
        }
        if plan.unchanged_contents.contains(&id) {
            continue;
        }
        let json = render_content(content);
        content_dir
            .write_atomic(&filename, json.as_bytes(), false)
            .map_err(|error| io_error(directory, error))?;
        contents.push((filename, json));
    }
    if plan.sweep {
        delete_unlisted(&content_dir, &directory.join("contents"), &keep_contents)?;
    } else {
        remove_named(&content_dir, directory, "contents", plan.removed_contents)?;
    }
    Ok(IncrementalText {
        project_json,
        tracks,
        contents,
    })
}

pub(crate) struct FilePlan<'a> {
    pub unchanged_tracks: &'a HashSet<String>,
    pub unchanged_contents: &'a HashSet<String>,
    pub previous_project_json: Option<&'a str>,
    pub removed_tracks: &'a [String],
    pub removed_contents: &'a [String],
    pub sweep: bool,
}

pub(crate) struct IncrementalText {
    pub project_json: String,
    pub tracks: Vec<(String, String)>,
    pub contents: Vec<(String, String)>,
}

fn remove_named(
    owned: &Directory,
    directory: &Path,
    folder: &str,
    ids: &[String],
) -> Result<(), PackageError> {
    for id in ids {
        let path = directory.join(folder).join(format!("{id}.json"));
        owned
            .remove(format!("{id}.json"))
            .map_err(|error| io_error(&path, error))?;
    }
    Ok(())
}

fn delete_unlisted(
    owned: &Directory,
    directory: &Path,
    keep: &HashSet<String>,
) -> Result<(), PackageError> {
    for entry in owned.names().map_err(|error| io_error(directory, error))? {
        let name = entry.to_string_lossy();
        if name.ends_with(".json") && !keep.contains(name.as_ref()) {
            owned
                .remove(&entry)
                .map_err(|error| io_error(directory, error))?;
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
    let package = repo
        .workdir()
        .ok_or_else(|| PackageError::new("package has no working directory for plugin state"))?;
    if header.track_ids.is_empty() {
        return assemble(header, contents, Vec::new(), package);
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
    assemble(header, contents, tracks, package)
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

/// Commits the tree and moves the autosave ref. `written` names the blobs the
/// caller wrote for this commit. Once this returns, the commit, its new
/// objects and the ref are durable, so the journal may drop the edit.
pub(crate) fn commit_tree(
    repo: &Repository,
    project_blob: ObjectId,
    content_blobs: &[(String, ObjectId)],
    track_blobs: &[(String, ObjectId)],
    parents: &[ObjectId],
    journal_seq: u64,
    written: &[ObjectId],
) -> Result<ObjectId, PackageError> {
    let mut written = written.to_vec();
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
        written.push(contents_tree);
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
        written.push(tracks_tree);
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
            commit_message(journal_seq),
            root,
            parents.iter().copied(),
        )
        .map_err(|error| PackageError::new(error.to_string()))?;
    let id = commit.detach();
    written.extend([root, id]);
    write_out_commit(repo, &written)?;
    // The chain's full sync also flushes the drive's cache over everything
    // written out above.
    remember_commit(repo, parents.first().copied(), id)?;
    Ok(id)
}

/// Writes out this commit's loose objects, their folders and the autosave
/// ref. gix renames them into place without syncing (XCA-01). An object
/// that is not loose was already stored, for example in a pack.
fn write_out_commit(repo: &Repository, objects: &[ObjectId]) -> Result<(), PackageError> {
    let git_dir = repo.git_dir();
    let failed = |error| io_error(git_dir, error);
    let git = Directory::open(git_dir).map_err(failed)?;
    let store = git.child("objects").map_err(failed)?;
    let mut folders = std::collections::BTreeMap::new();
    for id in objects {
        let hex = id.to_string();
        let (fan, name) = hex.split_at(2);
        if !folders.contains_key(fan) {
            let Some(folder) = store.optional_child(fan).map_err(failed)? else {
                continue;
            };
            folders.insert(fan.to_string(), folder);
        }
        let folder = &folders[fan];
        if folder.has_file(name).map_err(failed)? {
            let file = folder.open_file(name, false, false).map_err(failed)?;
            crate::package_io::write_out(&file).map_err(failed)?;
        }
    }
    for folder in folders.values() {
        folder.write_out().map_err(failed)?;
    }
    store.write_out().map_err(failed)?;
    let (folders, name) = AUTOSAVE_REF
        .rsplit_once('/')
        .expect("the autosave ref has folders");
    let mut folder = git;
    for part in folders.split('/') {
        folder.write_out().map_err(failed)?;
        folder = folder.child(part).map_err(failed)?;
    }
    folder.write_out().map_err(failed)?;
    let reference = folder.open_file(name, false, false).map_err(failed)?;
    crate::package_io::write_out(&reference).map_err(failed)
}

fn remember_commit(
    repo: &Repository,
    parent: Option<ObjectId>,
    id: ObjectId,
) -> Result<(), PackageError> {
    let path = repo.git_dir().join("echode-commit-chain");
    let parent = parent
        .map(|parent| parent.to_string())
        .unwrap_or_else(|| "-".to_string());
    let mut file = Directory::open(repo.git_dir())
        .and_then(|git| git.open_file("echode-commit-chain", true, true))
        .map_err(|error| io_error(&path, error))?;
    file.seek(SeekFrom::End(0))
        .map_err(|error| io_error(&path, error))?;
    std::io::Write::write_all(&mut file, format!("{id} {parent}\n").as_bytes())
        .map_err(|error| io_error(&path, error))?;
    file.sync_all().map_err(|error| io_error(&path, error))?;
    Ok(())
}

pub(crate) fn commit_parent(directory: &Path, id: ObjectId) -> Option<ObjectId> {
    let git = Directory::open(directory)
        .ok()?
        .optional_child(".git")
        .ok()??;
    let text = git.read_text("echode-commit-chain").ok()?;
    let id = id.to_string();
    text.lines().rev().find_map(|line| {
        let (commit, parent) = line.split_once(' ')?;
        (commit == id && parent != "-").then(|| ObjectId::from_hex(parent.as_bytes()).ok())?
    })
}

fn render_track(track: &Track) -> String {
    let mut out = track_json::render_header(track, SCHEMA);
    out.push_str("  \"devices\":[\n");
    for (index, device) in track.devices.iter().enumerate() {
        let comma = if index + 1 == track.devices.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{comma}", device_json::render(device));
    }
    out.push_str("  ],\n  \"clips\":[\n");
    push_json_lines(&mut out, "    ", track.clips.iter().map(to_json));
    out.push_str("  ]\n}\n");
    out
}

fn render_content(content: &echode_model::Content) -> String {
    #[derive(Serialize)]
    struct Header {
        schema: u32,
        id: Id,
        #[serde(flatten)]
        metadata: ContentMetadata,
    }
    let mut out = track_json::render_metadata(&Header {
        schema: SCHEMA,
        id: content.id,
        metadata: ContentMetadata::from(content),
    });
    out.push_str("  \"notes\":[\n");
    push_json_lines(&mut out, "    ", content.notes.iter().map(to_json));
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
    // Keep legacy bytes only when they still describe the original float.
    // Fixed decimal places lose quiet gains and low mantissa bits.
    if text.parse::<f32>() == Ok(gain) {
        text
    } else {
        gain.to_string()
    }
}

/// Current package schema. Older numbers are raised by [`SCHEMA_STEPS`].
const SCHEMA: u32 = 6;

/// Raises one schema number. Index 0 raises schema 1 to schema 2.
type SchemaStep = fn(&mut serde_json::Value) -> Result<(), String>;

/// Schema 1 devices have no plugin id. Schema 2 stores audio as null or an id.
/// Schema 3 devices were always active. Schema 5 devices pinned no parameters.
const SCHEMA_STEPS: &[SchemaStep] = &[
    add_device_plugin,
    audio_reference,
    add_device_bypass,
    add_device_offline,
    add_device_pins,
];

/// Earlier devices listed parameters in the plugin's order only.
fn add_device_pins(value: &mut serde_json::Value) -> Result<(), String> {
    let Some(devices) = value.get_mut("devices") else {
        return Ok(());
    };
    let Some(devices) = devices.as_array_mut() else {
        return Err("devices is not an array".into());
    };
    for device in devices {
        let Some(device) = device.as_object_mut() else {
            return Err("device is not an object".into());
        };
        device
            .entry("pinned")
            .or_insert(serde_json::Value::Array(Vec::new()));
    }
    Ok(())
}

/// Earlier devices always retained a runtime instance, including bypassed ones.
fn add_device_offline(value: &mut serde_json::Value) -> Result<(), String> {
    let Some(devices) = value.get_mut("devices") else {
        return Ok(());
    };
    let Some(devices) = devices.as_array_mut() else {
        return Err("devices is not an array".into());
    };
    for device in devices {
        let Some(device) = device.as_object_mut() else {
            return Err("device is not an object".into());
        };
        device
            .entry("offline")
            .or_insert(serde_json::Value::Bool(false));
    }
    Ok(())
}

/// Older devices were always active. The mixer cannot be bypassed.
fn add_device_bypass(value: &mut serde_json::Value) -> Result<(), String> {
    let Some(devices) = value.get_mut("devices") else {
        return Ok(());
    };
    let Some(devices) = devices.as_array_mut() else {
        return Err("devices is not an array".into());
    };
    for device in devices {
        let Some(device) = device.as_object_mut() else {
            return Err("device is not an object".into());
        };
        device
            .entry("bypassed")
            .or_insert(serde_json::Value::Bool(false));
    }
    Ok(())
}

fn add_device_plugin(value: &mut serde_json::Value) -> Result<(), String> {
    let Some(devices) = value.get_mut("devices") else {
        return Ok(());
    };
    let Some(devices) = devices.as_array_mut() else {
        return Err("devices is not an array".to_string());
    };
    for device in devices {
        let Some(object) = device.as_object_mut() else {
            return Err("device is not an object".to_string());
        };
        object.entry("plugin").or_insert(serde_json::Value::Null);
    }
    Ok(())
}

/// Schema 2 named audio with an object id. Those ids were never files, so they
/// become no audio. A hash object written by schema 3 is left alone.
fn audio_reference(value: &mut serde_json::Value) -> Result<(), String> {
    let Some(audio) = value.get_mut("audio") else {
        return Ok(());
    };
    if audio.is_string() {
        *audio = serde_json::Value::Null;
    }
    Ok(())
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

/// serde_json writes strings and structure. Floats retain the legacy spelling
/// when lossless, otherwise use round-trip decimals (ADR 0196). Zero stays `0`.
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
    metadata: ProjectMetadata,
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
    let metadata = serde_json::from_value(serde_json::Value::Object(file.metadata))
        .map_err(|error| PackageError::new(format!("project.json: {error}")))?;
    let tempo = TempoMap::try_from_points(file.tempo)
        .ok_or_else(|| PackageError::new("tempo map is empty or has two points at one tick"))?;
    Ok(Header {
        metadata,
        tempo,
        content_ids: canonical_track_ids(file.contents)?,
        track_ids: canonical_track_ids(file.tracks)?,
    })
}

fn assemble(
    header: Header,
    contents: Vec<(String, String)>,
    tracks: Vec<(String, String)>,
    package: &Path,
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
        parsed.push(parse_track(&json, id, package)?);
    }
    header
        .metadata
        .try_assemble(header.tempo, parsed_contents, parsed)
        .map_err(|error| PackageError::new(error.to_string()))
}

fn parse_track(json: &str, expected_id: &str, package: &Path) -> Result<Track, PackageError> {
    let file: TrackFile = parse_schema(json, "track json", "track")?;
    // Keep unknown-field rejection in the scalar type, rather than combining
    // Serde's unsupported flatten + deny_unknown_fields attributes.
    let metadata: TrackMetadata = serde_json::from_value(serde_json::Value::Object(file.metadata))
        .map_err(|error| PackageError::new(format!("track json: {error}")))?;
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
    let clips = file.clips;
    let devices = file
        .devices
        .into_iter()
        .map(|device| {
            let kind = device.kind;
            match (kind, device.state.as_deref()) {
                (_, None) => {}
                (echode_model::DeviceKind::Mixer, Some(_)) => {
                    return Err(PackageError::new("mixer state must be null"));
                }
                (echode_model::DeviceKind::Clap, Some(text)) => {
                    if !crate::objects::is_content_hash(text) {
                        return Err(PackageError::new(
                            "state hash must be 64 lowercase hex digits",
                        ));
                    }
                    crate::objects::read_object(package, text)?;
                }
            }
            match (kind, device.plugin.as_deref()) {
                (echode_model::DeviceKind::Mixer, None) => {}
                (echode_model::DeviceKind::Clap, Some(plugin))
                    if echode_model::valid_plugin_id(plugin) && device.parameters.is_empty() => {}
                _ => {
                    return Err(PackageError::new(format!(
                        "device {} does not match its plugin id",
                        device.id
                    )));
                }
            }
            Ok(device)
        })
        .collect::<Result<Vec<_>, PackageError>>()?;
    Ok(metadata.into_track(id, clips, devices))
}

fn parse_content(json: &str, expected_id: &str) -> Result<echode_model::Content, PackageError> {
    let file: ContentFile = parse_schema(json, "content json", "content")?;
    let metadata: ContentMetadata =
        serde_json::from_value(serde_json::Value::Object(file.metadata))
            .map_err(|error| PackageError::new(format!("content json: {error}")))?;
    if let Some(audio) = &metadata.audio {
        audio
            .check()
            .map_err(|error| PackageError::new(error.to_string()))?;
    }
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
    for note in &file.notes {
        if note.articulation.is_some() {
            return Err(PackageError::new("articulation is not accepted yet"));
        }
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
    Ok(metadata.into_content(parse_id(&file.id)?, file.notes, Vec::new()))
}

fn parse_id(text: &str) -> Result<Id, PackageError> {
    Id::parse(text).ok_or_else(|| PackageError::new(format!("invalid id {text}")))
}

fn commit_message(journal_seq: u64) -> String {
    format!("autosave\n\nEchode-Journal: {journal_seq}\n")
}

pub(crate) fn journal_seq_of(message: &str) -> u64 {
    message
        .lines()
        .find_map(|line| line.strip_prefix("Echode-Journal:"))
        .and_then(|rest| rest.trim().parse().ok())
        .unwrap_or(0)
}

pub(crate) fn next_journal_seq(directory: &Path) -> Result<u64, PackageError> {
    let disk = read_log(directory)?
        .into_iter()
        .map(|line| line.seq)
        .max()
        .unwrap_or(0);
    Ok(disk.max(tip_journal_seq(directory)).saturating_add(1))
}

fn tip_journal_seq(directory: &Path) -> u64 {
    let Some(id) = autosave_tip(directory) else {
        return 0;
    };
    let Ok(repo) = gix::open(directory) else {
        return 0;
    };
    let Ok(commit) = repo.find_commit(id) else {
        return 0;
    };
    let Ok(message) = commit.message_raw() else {
        return 0;
    };
    journal_seq_of(&String::from_utf8_lossy(message))
}

fn read_log(directory: &Path) -> Result<Vec<echode_actions::JournalLine>, PackageError> {
    let root = open_package(directory)?;
    let path = directory.join("journal.jsonl");
    if !root
        .has_file("journal.jsonl")
        .map_err(|error| io_error(&path, error))?
    {
        return Ok(Vec::new());
    }
    let bytes = root
        .read("journal.jsonl")
        .map_err(|error| io_error(&path, error))?;
    let text = std::str::from_utf8(committed_journal(&bytes))
        .map_err(|error| PackageError::new(error.to_string()))?;
    let mut records = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        records.push(
            echode_actions::decode_line(line)
                .map_err(|error| PackageError::new(error.to_string()))?,
        );
    }
    Ok(records)
}

fn contiguous_suffix(
    base_seq: u64,
    records: &[echode_actions::JournalLine],
) -> Vec<&echode_actions::JournalLine> {
    let mut suffix: Vec<_> = records.iter().filter(|line| line.seq > base_seq).collect();
    suffix.sort_by_key(|line| line.seq);
    let mut expected = base_seq.saturating_add(1);
    let mut count = 0usize;
    for line in &suffix {
        if line.seq != expected {
            break;
        }
        expected = expected.saturating_add(1);
        count += 1;
    }
    suffix.truncate(count);
    suffix
}

/// A replayed project, the last journal line in it, and why later lines are not.
struct Replayed {
    project: Project,
    applied: u64,
    stopped: Option<RecoveryReason>,
}

/// Replays the journal lines that continue `base_seq`. It stops before a line
/// whose plugin state the store does not hold intact (HIS-A1), and it reports
/// lines left after a missing one (XCA-02). Either way the document must not
/// look complete: the next autosave would commit it as the tip.
fn replay_suffix(
    directory: &Path,
    project: Project,
    base_seq: u64,
    records: &[echode_actions::JournalLine],
) -> Result<Replayed, PackageError> {
    let suffix = contiguous_suffix(base_seq, records);
    let mut project = project;
    let mut applied = base_seq;
    for line in suffix {
        let changes = match line.op {
            echode_actions::JournalOp::Undo => line.changes.invert(),
            echode_actions::JournalOp::Edit | echode_actions::JournalOp::Redo => {
                line.changes.clone()
            }
        };
        let before = project;
        project = before
            .apply_changeset(&changes)
            .map_err(|error| PackageError::new(error.to_string()))?;
        // A journal is external input, not a trusted in-memory undo step.
        // Check each complete transaction before accepting any later line.
        project.check_invariants().map_err(|error| {
            PackageError::new(format!(
                "journal seq {} violates model invariants: {error}",
                line.seq
            ))
        })?;
        if !states_stored(directory, &before, &project) {
            return Ok(Replayed {
                project: before,
                applied,
                stopped: Some(RecoveryReason::MissingState),
            });
        }
        applied = line.seq;
    }
    let gap = records.iter().any(|line| line.seq > applied);
    Ok(Replayed {
        project,
        applied,
        stopped: gap.then_some(RecoveryReason::JournalGap),
    })
}

/// True when the store holds intact every plugin state `after` names that
/// `before` did not. States read from a commit were checked by `parse_track`.
fn states_stored(directory: &Path, before: &Project, after: &Project) -> bool {
    let known: std::collections::HashMap<Id, Option<&str>> = before
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .map(|device| (device.id, device.state.as_deref()))
        .collect();
    after
        .tracks()
        .iter()
        .flat_map(|track| &track.devices)
        .all(|device| match device.state.as_deref() {
            None => true,
            Some(hash) => {
                known.get(&device.id).copied().flatten() == Some(hash)
                    || crate::objects::read_object(directory, hash).is_ok()
            }
        })
}

fn first_readable_commit(
    directory: &Path,
    tip: ObjectId,
) -> Result<Option<(ObjectId, Project, u64)>, PackageError> {
    let repo = gix::open(directory).map_err(|error| PackageError::new(error.to_string()))?;
    let mut cursor = Some(tip);
    while let Some(id) = cursor {
        let chained = commit_parent(directory, id);
        let loaded = repo.find_commit(id).ok().and_then(|commit| {
            let message = commit
                .message_raw()
                .map(|message| String::from_utf8_lossy(message).into_owned())
                .unwrap_or_default();
            read_commit(&repo, id)
                .ok()
                .map(|project| (project, journal_seq_of(&message)))
        });
        if let Some((project, seq)) = loaded {
            return Ok(Some((id, project, seq)));
        }
        cursor = chained;
    }
    Ok(None)
}

/// The autosave tip: the ref, or a newer commit the chain records on top of
/// it. Every commit syncs its chain line, while the ref's update can be lost
/// with the drive's cache. Recovery then goes on from the newest commit
/// instead of hiding it behind an older ref (XCA-02).
pub(crate) fn autosave_tip(directory: &Path) -> Option<ObjectId> {
    let repo = gix::open(directory).ok()?;
    let reference = repo.try_find_reference(AUTOSAVE_REF).ok()??.id().detach();
    Some(chained_descendant(directory, reference).unwrap_or(reference))
}

/// The chain's newest commit, when the chain leads from it back to `tip`.
fn chained_descendant(directory: &Path, tip: ObjectId) -> Option<ObjectId> {
    let git = Directory::open(directory)
        .ok()?
        .optional_child(".git")
        .ok()??;
    let text = git.read_text("echode-commit-chain").ok()?;
    let parents: std::collections::HashMap<&str, &str> = text
        .lines()
        .filter_map(|line| line.split_once(' '))
        .collect();
    let newest = text.lines().last()?.split_once(' ')?.0;
    let tip = tip.to_string();
    let mut cursor = newest;
    // Each step follows a recorded parent; a cycle cannot outlast the chain.
    for _ in 0..=parents.len() {
        if cursor == tip {
            return (newest != tip)
                .then(|| ObjectId::from_hex(newest.as_bytes()).ok())
                .flatten();
        }
        cursor = parents.get(cursor)?;
    }
    None
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

fn canonical_track_ids(ids: Vec<String>) -> Result<Vec<String>, PackageError> {
    ids.into_iter()
        .map(|id| {
            Id::parse(&id)
                .map(|id| id.to_string())
                .ok_or_else(|| PackageError::new(format!("invalid track id {id}")))
        })
        .collect()
}

pub(crate) fn io_error(path: &Path, error: std::io::Error) -> PackageError {
    PackageError::new(format!("{}: {error}", path.display()))
}

#[derive(Deserialize)]
struct ProjectFile {
    schema: u32,
    #[serde(flatten)]
    metadata: serde_json::Map<String, serde_json::Value>,
    tempo: Vec<TempoPoint>,
    #[serde(default)]
    contents: Vec<String>,
    tracks: Vec<String>,
}

#[derive(Deserialize)]
struct TrackFile {
    schema: u32,
    id: String,
    #[serde(flatten)]
    metadata: serde_json::Map<String, serde_json::Value>,
    devices: Vec<echode_model::Device>,
    clips: Vec<Placement>,
}

#[derive(Deserialize)]
struct ContentFile {
    schema: u32,
    id: String,
    #[serde(flatten)]
    metadata: serde_json::Map<String, serde_json::Value>,
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
        // Do not churn existing canonical bytes when the old spelling was exact.
        assert_eq!(format_gain(0.1), "0.100000001");
        assert_eq!(format_gain(0.0001), "0.0001");
    }

    #[test]
    fn finite_nonzero_floats_keep_every_bit_in_package_json() {
        #[derive(Serialize)]
        struct Row {
            gain: f32,
        }
        // A deterministic sample across exponent/mantissa/sign bits, not a model limit.
        let mut bits = 1_u32;
        for _ in 0..10_000 {
            let value = f32::from_bits(bits);
            if value.is_finite() && value != 0.0 {
                let parsed: serde_json::Value =
                    serde_json::from_str(&to_json(&Row { gain: value })).unwrap();
                let restored: f32 = serde_json::from_value(parsed["gain"].clone()).unwrap();
                assert_eq!(
                    restored.to_bits(),
                    bits,
                    "float spelling lost bits: {value:e}"
                );
            }
            bits = bits.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
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
