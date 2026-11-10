//! Thin an autosave chain and pack the objects that remain.
//!
//! gix 0.78 can build a pack and its index. It has no gc, prune, or git-bundle
//! writer. This module keeps the §6.4 schedule, rewrites the kept commits so
//! dropped parents are not reachable, and deletes everything else.

use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use gix::objs::tree::{Entry, EntryKind};
use gix::{ObjectId, Repository};

use crate::document::AUTOSAVE_REF;
use crate::error::{GitOperation, PackageError, ThinningError};

/// Seconds. Commits newer than this stay.
pub const RECENT: i64 = 24 * 60 * 60;
/// Seconds. Between [`RECENT`] and this, one commit per hour stays.
pub const HOURLY_SPAN: i64 = 30 * 24 * 60 * 60;
/// Seconds in one hour.
pub const HOUR: i64 = 60 * 60;
/// Seconds in one day.
pub const DAY: i64 = 24 * 60 * 60;

/// One autosave commit, oldest first. Named saves are not in this list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavePoint {
    pub id: ObjectId,
    pub time: i64,
}

/// Files and bytes under `.git`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreSize {
    pub files: u64,
    pub bytes: u64,
}

/// Indexes to keep. `times` is oldest first. Within an hour or a day, the
/// newest commit in that bucket stays. A named save is omitted by the caller.
pub fn kept_indexes(times: &[i64], now: i64) -> Vec<usize> {
    let mut keep = Vec::new();
    let mut hours = HashSet::new();
    let mut days = HashSet::new();
    for (index, time) in times.iter().enumerate().rev() {
        let age = now.saturating_sub(*time);
        if age <= RECENT {
            keep.push(index);
        } else if age <= HOURLY_SPAN {
            if hours.insert(time.div_euclid(HOUR)) {
                keep.push(index);
            }
        } else if days.insert(time.div_euclid(DAY)) {
            keep.push(index);
        }
    }
    keep.reverse();
    keep
}

pub fn git_store_size(git_dir: &Path) -> StoreSize {
    let mut size = StoreSize { files: 0, bytes: 0 };
    walk(git_dir, &mut size);
    size
}

/// Rewrite `points` down to `kept` indexes, pack the objects those commits
/// still need, and drop the rest. Returns the new tip.
pub fn pack_and_thin(
    repo: &Repository,
    points: &[SavePoint],
    kept: &[usize],
) -> Result<ObjectId, PackageError> {
    let tip = rewrite_chain(repo, points, kept)?;
    repack_reachable(repo)?;
    Ok(tip)
}

fn rewrite_chain(
    repo: &Repository,
    points: &[SavePoint],
    kept: &[usize],
) -> Result<ObjectId, PackageError> {
    let mut parent = None;
    let mut tip = None;
    for index in kept {
        let point = points[*index];
        let commit = repo
            .find_object(point.id)
            .map_err(|error| PackageError::git(GitOperation::FindObject, error))?
            .try_into_commit()
            .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
        let tree = commit
            .decode()
            .map_err(|error| PackageError::git(GitOperation::DecodeCommit, error))?
            .tree();
        let stored;
        let parents: &[ObjectId] = if let Some(id) = parent {
            stored = [id];
            &stored
        } else {
            &[]
        };
        let id = write_commit(repo, tree, parents, point.time)?;
        parent = Some(id);
        tip = Some(id);
    }
    let tip = tip.ok_or(ThinningError::NothingToKeep)?;
    let mut reference = repo
        .find_reference(AUTOSAVE_REF)
        .map_err(|error| PackageError::git(GitOperation::FindReference, error))?;
    reference
        .set_target_id(tip, "thin autosave")
        .map_err(|error| PackageError::git(GitOperation::UpdateReference, error))?;
    Ok(tip)
}

fn write_commit(
    repo: &Repository,
    tree: ObjectId,
    parents: &[ObjectId],
    time: i64,
) -> Result<ObjectId, PackageError> {
    let signature = gix::actor::Signature {
        name: "Echode".into(),
        email: "history@echode.local".into(),
        time: gix::date::Time::new(time, 0),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: parents.iter().copied().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!("autosave {time}\n").into(),
        extra_headers: Vec::new(),
    };
    Ok(repo
        .write_object(&commit)
        .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
        .detach())
}

fn commit_at(
    repo: &Repository,
    tree: ObjectId,
    parents: &[ObjectId],
    time: i64,
) -> Result<ObjectId, PackageError> {
    let signature = gix::actor::Signature {
        name: "Echode".into(),
        email: "history@echode.local".into(),
        time: gix::date::Time::new(time, 0),
    };
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let commit = repo
        .commit_as(
            signature.to_ref(&mut committer_time),
            signature.to_ref(&mut author_time),
            AUTOSAVE_REF,
            format!("autosave {time}"),
            tree,
            parents.iter().copied(),
        )
        .map_err(|error| PackageError::git(GitOperation::Commit, error))?;
    Ok(commit.detach())
}

fn repack_reachable(repo: &Repository) -> Result<(), PackageError> {
    let mut reachable = HashSet::new();
    for tip in reference_tips(repo)? {
        collect(repo, tip, &mut reachable)?;
    }
    let pack = encode_pack(repo, &reachable)?;
    let pack_dir = repo.git_dir().join("objects").join("pack");
    fs::create_dir_all(&pack_dir)?;
    let mut progress = gix::progress::Discard;
    let interrupt = AtomicBool::new(false);
    let outcome = gix_pack::Bundle::write_to_directory(
        &mut Cursor::new(pack),
        Some(pack_dir.as_path()),
        &mut progress,
        &interrupt,
        None::<gix::objs::find::Never>,
        repo.object_hash(),
        gix_pack::bundle::write::Options::default(),
    )
    .map_err(|error| PackageError::git(GitOperation::WritePack, error))?;
    if let Some(path) = outcome.keep_path {
        let _ = fs::remove_file(path);
    }
    let keep_data = outcome.data_path;
    let keep_index = outcome.index_path;
    if pack_dir.is_dir() {
        for entry in fs::read_dir(&pack_dir)? {
            let entry = entry?;
            let path = entry.path();
            if Some(&path) == keep_data.as_ref() || Some(&path) == keep_index.as_ref() {
                continue;
            }
            if path.is_file() {
                fs::remove_file(&path)?;
            }
        }
    }
    delete_loose(repo.git_dir().join("objects").as_path())?;
    Ok(())
}

fn reference_tips(repo: &Repository) -> Result<Vec<ObjectId>, PackageError> {
    let mut tips = Vec::new();
    let platform = repo
        .references()
        .map_err(|error| PackageError::git(GitOperation::References, error))?;
    for reference in platform
        .all()
        .map_err(|error| PackageError::git(GitOperation::IterateReferences, error))?
    {
        let reference =
            reference.map_err(|error| PackageError::git(GitOperation::ReadReference, error))?;
        if let Some(id) = reference.target().try_id() {
            tips.push(id.to_owned());
        }
    }
    if tips.is_empty() {
        return Err(ThinningError::NoReferenceTips.into());
    }
    Ok(tips)
}

fn collect(
    repo: &Repository,
    id: ObjectId,
    seen: &mut HashSet<ObjectId>,
) -> Result<(), PackageError> {
    if !seen.insert(id) {
        return Ok(());
    }
    let object = repo
        .find_object(id)
        .map_err(|error| PackageError::git(GitOperation::FindObject, error))?;
    match object.kind {
        gix::objs::Kind::Commit => {
            let commit = object
                .try_into_commit()
                .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
            let decoded = commit
                .decode()
                .map_err(|error| PackageError::git(GitOperation::DecodeCommit, error))?;
            let tree = decoded.tree();
            let parents: Vec<ObjectId> = decoded.parents().collect();
            drop(decoded);
            collect(repo, tree, seen)?;
            for parent in parents {
                collect(repo, parent, seen)?;
            }
        }
        gix::objs::Kind::Tree => {
            let tree = object
                .try_into_tree()
                .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
            let decoded = tree
                .decode()
                .map_err(|error| PackageError::git(GitOperation::DecodeTree, error))?;
            let children: Vec<ObjectId> = decoded
                .entries
                .iter()
                .map(|entry| ObjectId::from(entry.oid))
                .collect();
            drop(decoded);
            for child in children {
                collect(repo, child, seen)?;
            }
        }
        gix::objs::Kind::Blob | gix::objs::Kind::Tag => {}
    }
    Ok(())
}

fn encode_pack(repo: &Repository, ids: &HashSet<ObjectId>) -> Result<Vec<u8>, PackageError> {
    let mut body = Vec::new();
    body.extend_from_slice(b"PACK");
    body.extend_from_slice(&2u32.to_be_bytes());
    body.extend_from_slice(&(ids.len() as u32).to_be_bytes());
    for id in ids {
        let object = repo
            .find_object(*id)
            .map_err(|error| PackageError::git(GitOperation::FindObject, error))?;
        let kind = match object.kind {
            gix::objs::Kind::Commit => 1,
            gix::objs::Kind::Tree => 2,
            gix::objs::Kind::Blob => 3,
            gix::objs::Kind::Tag => 4,
        };
        body.extend(pack_header(kind, object.data.len()));
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&object.data)?;
        body.extend(encoder.finish()?);
    }
    let mut hasher = gix::hash::hasher(repo.object_hash());
    hasher.update(&body);
    let digest = hasher
        .try_finalize()
        .map_err(|error| PackageError::git(GitOperation::HashPack, error))?;
    body.extend_from_slice(digest.as_bytes());
    Ok(body)
}

fn pack_header(kind: u8, mut size: usize) -> Vec<u8> {
    let mut byte = (kind << 4) | ((size as u8) & 0x0f);
    size >>= 4;
    let mut header = Vec::new();
    while size > 0 {
        header.push(byte | 0x80);
        byte = (size as u8) & 0x7f;
        size >>= 7;
    }
    header.push(byte);
    header
}

fn delete_loose(objects: &Path) -> Result<(), PackageError> {
    let Ok(fans) = fs::read_dir(objects) else {
        return Ok(());
    };
    for fan in fans {
        let fan = fan?;
        let name = fan.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() != 2 || !name.chars().all(|ch| ch.is_ascii_hexdigit()) {
            continue;
        }
        fs::remove_dir_all(fan.path())?;
    }
    Ok(())
}

fn walk(path: &Path, size: &mut StoreSize) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, size);
        } else if let Ok(meta) = entry.metadata() {
            size.files += 1;
            size.bytes += meta.len();
        }
    }
}

/// One changing file per commit, so each save adds a blob, a tree, and a commit.
pub fn seed_autosaves(
    directory: &Path,
    count: usize,
    step: i64,
    origin: i64,
) -> Result<Vec<SavePoint>, PackageError> {
    let repo =
        gix::init(directory).map_err(|error| PackageError::git(GitOperation::Init, error))?;
    let mut parent = None;
    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        let time = origin + index as i64 * step;
        let blob = repo
            .write_object(&gix::objs::Blob {
                data: format!("save {index}").into_bytes(),
            })
            .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
            .detach();
        let tree = repo
            .write_object(&gix::objs::Tree {
                entries: vec![Entry {
                    mode: EntryKind::Blob.into(),
                    filename: "project.json".into(),
                    oid: blob,
                }],
            })
            .map_err(|error| PackageError::git(GitOperation::WriteObject, error))?
            .detach();
        let stored;
        let parents: &[ObjectId] = if let Some(id) = parent {
            stored = [id];
            &stored
        } else {
            &[]
        };
        let id = commit_at(&repo, tree, parents, time)?;
        parent = Some(id);
        points.push(SavePoint { id, time });
    }
    Ok(points)
}

pub fn tip_text(repo: &Repository) -> Result<String, PackageError> {
    let reference = repo
        .find_reference(AUTOSAVE_REF)
        .map_err(|error| PackageError::git(GitOperation::FindReference, error))?;
    let id = reference
        .target()
        .try_id()
        .ok_or(ThinningError::SymbolicAutosave)?
        .to_owned();
    let commit = repo
        .find_object(id)
        .map_err(|error| PackageError::git(GitOperation::FindObject, error))?
        .try_into_commit()
        .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
    let tree_id = commit
        .decode()
        .map_err(|error| PackageError::git(GitOperation::DecodeCommit, error))?
        .tree();
    let tree = repo
        .find_object(tree_id)
        .map_err(|error| PackageError::git(GitOperation::FindObject, error))?
        .try_into_tree()
        .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
    let decoded = tree
        .decode()
        .map_err(|error| PackageError::git(GitOperation::DecodeTree, error))?;
    let entry = decoded
        .entries
        .iter()
        .find(|entry| entry.filename == "project.json")
        .ok_or(ThinningError::MissingProject)?;
    let blob_id = ObjectId::from(entry.oid);
    let blob = repo
        .find_object(blob_id)
        .map_err(|error| PackageError::git(GitOperation::FindObject, error))?
        .try_into_blob()
        .map_err(|error| PackageError::git(GitOperation::ConvertObject, error))?;
    let bytes = blob.data.clone();
    String::from_utf8(bytes).map_err(|error| ThinningError::Text(error).into())
}

/// A bundle is a header in front of a pack. gix does not write that header.
pub fn write_bundle(repo: &Repository, tip: ObjectId, path: &Path) -> Result<u64, PackageError> {
    let mut reachable = HashSet::new();
    collect(repo, tip, &mut reachable)?;
    let pack = encode_pack(repo, &reachable)?;
    let mut file = Vec::new();
    write!(file, "# v2 git bundle\n{tip} {AUTOSAVE_REF}\n\n")?;
    file.extend(pack);
    fs::write(path, &file)?;
    Ok(file.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_commits_stay_and_older_hours_keep_one() {
        let origin = 1_700_000_000;
        let mut times = Vec::new();
        for index in 0..6 {
            times.push(origin + index * 10);
        }
        let now = *times.last().unwrap();
        assert_eq!(kept_indexes(&times, now), (0..6).collect::<Vec<_>>());
        let later = now + RECENT + HOUR;
        let kept = kept_indexes(&times, later);
        assert!(kept.len() < times.len());
        assert_eq!(*kept.last().unwrap(), times.len() - 1);
    }

    #[test]
    fn no_reference_tips_is_a_typed_refusal() {
        let directory =
            std::env::temp_dir().join(format!("echode-no-reference-tips-{}", std::process::id()));
        let repo = gix::init(&directory).unwrap();
        let error = reference_tips(&repo).unwrap_err();
        assert!(matches!(
            error,
            PackageError::Thinning(ThinningError::NoReferenceTips)
        ));
        assert_eq!(error.to_string(), "the repository has no commit to keep");
        fs::remove_dir_all(directory).unwrap();
    }
}
