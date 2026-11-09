//! Git object reads and durable autosave commits for product packages.

use std::io::{Seek, SeekFrom};

use echode_model::Project;
use gix::objs::Tree;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};

use super::{AUTOSAVE_REF, assemble, commit_message, io_error, parse_header};
use crate::error::{GitOperation, PackageError};
use crate::package_io::Directory;

pub(crate) fn read_commit(repo: &Repository, revision: ObjectId) -> Result<Project, PackageError> {
    let commit = repo
        .find_commit(revision)
        .map_err(|error| PackageError::git(GitOperation::FindObject, error))?;
    let root = commit
        .tree()
        .map_err(|error| PackageError::git(GitOperation::ReadCommitTree, error))?;
    let project_json = blob_text(&root)?;
    let header = parse_header(&project_json)?;
    let contents = read_tree_files(&root, "contents", &header.content_ids)?;
    let package = repo
        .workdir()
        .ok_or(PackageError::MissingWorkingDirectory)?;
    let tracks = read_tree_files(&root, "tracks", &header.track_ids)?;
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
        .map_err(|error| PackageError::git(GitOperation::LookupEntry, error))?
        .ok_or_else(|| PackageError::MissingRevisionDirectory(folder.to_string()))?;
    let tree = entry
        .object()
        .map_err(|error| PackageError::git(GitOperation::ReadObject, error))?
        .try_into_tree()
        .map_err(|error| PackageError::git(GitOperation::ObjectKind, error))?;
    let mut by_name = std::collections::HashMap::with_capacity(ids.len());
    for entry in tree.iter() {
        let entry = entry.map_err(|error| PackageError::git(GitOperation::DecodeTree, error))?;
        let filename = entry.filename().to_string();
        let blob = entry
            .object()
            .map_err(|error| PackageError::git(GitOperation::ReadObject, error))?
            .try_into_blob()
            .map_err(|error| PackageError::git(GitOperation::ObjectKind, error))?;
        let json = std::str::from_utf8(&blob.data)
            .map_err(PackageError::RevisionText)?
            .to_string();
        by_name.insert(filename, json);
    }
    let mut files = Vec::with_capacity(ids.len());
    for id in ids {
        let filename = format!("{id}.json");
        let json = by_name
            .remove(&filename)
            .ok_or_else(|| PackageError::MissingRevisionFile(format!("{folder}/{filename}")))?;
        files.push((filename, json));
    }
    Ok(files)
}

fn blob_text(root: &gix::Tree<'_>) -> Result<String, PackageError> {
    let entry = root
        .lookup_entry_by_path("project.json")
        .map_err(|error| PackageError::git(GitOperation::LookupEntry, error))?
        .ok_or(PackageError::MissingRevisionProject)?;
    let blob = entry
        .object()
        .map_err(|error| PackageError::git(GitOperation::ReadObject, error))?
        .try_into_blob()
        .map_err(|error| PackageError::git(GitOperation::ObjectKind, error))?;
    std::str::from_utf8(&blob.data)
        .map_err(PackageError::RevisionText)
        .map(ToOwned::to_owned)
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
            .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
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
            .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
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
        .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
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
        .map_err(|error| PackageError::git(GitOperation::Commit, error))?;
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
