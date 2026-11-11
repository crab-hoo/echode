//! Content-addressed bytes for audio and plugin state.
//!
//! A file in `media/objects/` is named by the BLAKE3-256 of its bytes. Git
//! commits do not include that directory.

use std::collections::HashSet;
#[cfg(test)]
use std::fs;
use std::path::Path;

use crate::document::{PackageError, io_error};
use crate::package_io::Directory;

const GITIGNORE: &str = "/media/objects/\n/cache/\n";

/// Lowercase BLAKE3-256 hex, the only spelling the package accepts.
pub(crate) fn is_content_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// BLAKE3-256 of `bytes`, as 64 lowercase hex digits: the object's name.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Writes `bytes` to `media/objects/<hash>`. An object that already matches is
/// kept. One whose bytes no longer match its name is damaged and is replaced.
///
/// The object and its directory entry are synced before this returns: the
/// journal line that names the object is written next, and Git commits do not
/// hold the store (XCA-03).
pub fn write_object(package: &Path, bytes: &[u8]) -> Result<String, PackageError> {
    stage_object(package, bytes)?
        .publish(package)
        .map(|published| published.hash)
}

/// Bytes written beside `media/objects/<hash>` that the store does not name yet.
///
/// An import stages its PCM before the edit that places it is checked. Only an
/// accepted edit publishes the object; dropping the stage removes the
/// temporary entry, so a rejected import leaves nothing in the store (EXT-02).
pub(crate) struct StagedObject {
    /// Taken by [`Self::publish`]; `Drop` then has nothing to remove.
    objects: Option<Directory>,
    hash: String,
    /// `None` when the store already held matching bytes.
    temporary: Option<String>,
}

impl StagedObject {
    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }

    /// Renames the staged bytes to their hash and syncs the directory entry,
    /// which the journal line written next relies on (XCA-03).
    pub(crate) fn publish(mut self, package: &Path) -> Result<PublishedObject, PackageError> {
        let hash = std::mem::take(&mut self.hash);
        let (Some(objects), Some(temporary)) = (self.objects.take(), self.temporary.take()) else {
            return Ok(PublishedObject {
                created: None,
                hash,
            });
        };
        let directory = package.join("media").join("objects");
        let renamed = objects.has_file(&hash).and_then(|present| {
            objects.rename_into(&temporary, &hash)?;
            Ok(!present)
        });
        let created = match renamed {
            Ok(created) => created,
            Err(error) => {
                let _ = objects.remove(&temporary);
                return Err(io_error(&directory, error));
            }
        };
        if let Err(error) = objects.sync() {
            if created {
                let _ = objects.remove(&hash);
            }
            return Err(io_error(&directory, error));
        }
        Ok(PublishedObject {
            created: created.then_some(objects),
            hash,
        })
    }
}

/// A store entry named by an accepted stage.
pub(crate) struct PublishedObject {
    /// The store directory, kept only when publishing created the name.
    created: Option<Directory>,
    hash: String,
}

impl PublishedObject {
    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }

    /// Removes the name again when the edit that needed it failed after
    /// publishing. An entry that existed before the stage is kept.
    pub(crate) fn withdraw(self) {
        if let Some(objects) = self.created {
            let _ = objects.remove(&self.hash);
        }
    }
}

impl Drop for StagedObject {
    fn drop(&mut self) {
        if let (Some(objects), Some(temporary)) = (&self.objects, self.temporary.take()) {
            let _ = objects.remove(temporary);
        }
    }
}

/// Writes `bytes` beside the store, synced, without naming them yet. A store
/// entry that already matches is kept; a damaged one is replaced on publish.
pub(crate) fn stage_object(package: &Path, bytes: &[u8]) -> Result<StagedObject, PackageError> {
    let hash = hash_bytes(bytes);
    let directory = package.join("media").join("objects");
    let objects = Directory::create(package)
        .and_then(|root| root.child("media"))
        .and_then(|media| media.child("objects"))
        .map_err(|error| io_error(&directory, error))?;
    let current = objects
        .has_file(&hash)
        .map_err(|error| io_error(&directory, error))?
        && objects
            .read(&hash)
            .map_err(|error| io_error(&directory, error))?
            == bytes;
    let temporary = if current {
        None
    } else {
        Some(
            objects
                .write_temporary(&hash, bytes, true)
                .map_err(|error| io_error(&directory, error))?,
        )
    };
    Ok(StagedObject {
        objects: Some(objects),
        hash,
        temporary,
    })
}

/// Writes the ignore rules for the object store. Existing rules are kept.
pub(crate) fn ensure_store_gitignore(directory: &Path) -> Result<(), PackageError> {
    let root = Directory::create(directory).map_err(|error| io_error(directory, error))?;
    root.validate_layout()
        .map_err(|error| io_error(directory, error))?;
    let path = directory.join(".gitignore");
    if root
        .has_file(".gitignore")
        .map_err(|error| io_error(&path, error))?
    {
        let current = root
            .read_text(".gitignore")
            .map_err(|error| io_error(&path, error))?;
        let has_rule = |rule: &str| current.lines().any(|line| line == rule);
        let has_objects = has_rule("/media/objects/");
        let has_cache = has_rule("/cache/");
        if has_objects && has_cache {
            return Ok(());
        }
        let mut next = current;
        if !next.ends_with('\n') {
            next.push('\n');
        }
        if !has_objects {
            next.push_str("/media/objects/\n");
        }
        if !has_cache {
            next.push_str("/cache/\n");
        }
        root.write_atomic(".gitignore", next.as_bytes(), false)
            .map_err(|error| io_error(&path, error))?;
        return Ok(());
    }
    root.write_atomic(".gitignore", GITIGNORE.as_bytes(), false)
        .map_err(|error| io_error(&path, error))
}

/// Reads `media/objects/<hash>` after checking the name, the link, and the bytes.
pub fn read_object(package: &Path, hash: &str) -> Result<Vec<u8>, PackageError> {
    if !is_content_hash(hash) {
        return Err(PackageError::InvalidObjectHash);
    }
    let path = package.join("media").join("objects").join(hash);
    let bytes = Directory::open(package)
        .and_then(|root| root.required_child("media"))
        .and_then(|media| media.required_child("objects"))
        .and_then(|objects| objects.read(hash))
        .map_err(|error| io_error(&path, error))?;
    if hash_bytes(&bytes) != hash {
        return Err(PackageError::ObjectHashMismatch { hash: hash.into() });
    }
    Ok(bytes)
}

/// Names in the store that the current package and the remaining commits do not use.
///
/// Nothing deletes files yet. Plugin hosting will pass the hashes those commits name.
#[allow(dead_code)]
pub(crate) fn unreferenced(present: &[String], referenced: &HashSet<String>) -> Vec<String> {
    present
        .iter()
        .filter(|hash| !referenced.contains(hash.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BLAKE3-256 of empty bytes. The official test vector.
    const EMPTY_HASH: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    #[test]
    fn audit_pro01_object_store_directory_links_are_not_followed() {
        for folder in ["media", "objects"] {
            let root = std::env::temp_dir().join(format!(
                "echode-object-links-{}-{folder}",
                std::process::id()
            ));
            let package = root.join("project.echode");
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::create_dir_all(package.join("media")).unwrap();
            fs::write(outside.join("sentinel"), b"external").unwrap();
            let linked = if folder == "media" {
                fs::remove_dir(package.join("media")).unwrap();
                package.join("media")
            } else {
                package.join("media/objects")
            };
            std::os::unix::fs::symlink(&outside, linked).unwrap();
            assert!(write_object(&package, b"new audio").is_err());
            assert!(read_object(&package, EMPTY_HASH).is_err());
            assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"external");
            assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn empty_bytes_match_the_blake3_vector() {
        assert_eq!(hash_bytes(b""), EMPTY_HASH);
        assert!(is_content_hash(EMPTY_HASH));
        assert!(!is_content_hash(&EMPTY_HASH.to_ascii_uppercase()));
        assert!(!is_content_hash("abc"));
    }

    #[test]
    fn an_object_must_be_the_bytes_it_names() {
        let package = std::env::temp_dir().join(format!(
            "echode-objects-{}-{}",
            std::process::id(),
            EMPTY_HASH
        ));
        let _ = fs::remove_dir_all(&package);
        let objects = package.join("media").join("objects");
        fs::create_dir_all(&objects).unwrap();
        let good = objects.join(EMPTY_HASH);
        fs::write(&good, b"").unwrap();
        assert_eq!(read_object(&package, EMPTY_HASH).unwrap(), b"");

        let other = hash_bytes(b"hello");
        fs::write(objects.join(&other), b"nope").unwrap();
        let mismatch = read_object(&package, &other).unwrap_err().to_string();
        assert!(mismatch.contains("does not match"), "{mismatch}");
        // The same content repairs a damaged object (XCA-03).
        assert_eq!(write_object(&package, b"hello").unwrap(), other);
        assert_eq!(read_object(&package, &other).unwrap(), b"hello");
        let named = read_object(&package, &other.to_ascii_uppercase())
            .unwrap_err()
            .to_string();
        assert!(named.starts_with("object hash"), "{named}");

        let outside = package.join("outside.bin");
        fs::write(&outside, b"").unwrap();
        fs::remove_file(&good).unwrap();
        std::os::unix::fs::symlink(&outside, &good).unwrap();
        let link = read_object(&package, EMPTY_HASH).unwrap_err().to_string();
        let _ = fs::remove_dir_all(&package);
        assert!(link.contains("symbolic link"), "{link}");
    }

    #[test]
    fn only_unreferenced_objects_are_removal_candidates() {
        let keep = hash_bytes(b"keep");
        let drop = hash_bytes(b"drop");
        let present = vec![keep.clone(), drop.clone()];
        let referenced = HashSet::from([keep.clone()]);
        assert_eq!(unreferenced(&present, &referenced), vec![drop]);
    }
}
