//! Content-addressed bytes for audio and plugin state.
//!
//! A file in `media/objects/` is named by the BLAKE3-256 of its bytes. Git
//! commits do not include that directory.

use std::collections::HashSet;
#[cfg(test)]
use std::fs;
use std::path::Path;

use crate::document::PackageError;
use crate::package_io::Directory;

const GITIGNORE: &str = "/media/objects/\n/cache/\n";

/// Lowercase BLAKE3-256 hex, the only spelling the package accepts.
pub(crate) fn is_content_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// BLAKE3-256 of `bytes`, as 64 lowercase hex digits.
pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Writes `bytes` to `media/objects/<hash>`. An object that already matches is kept.
pub fn write_object(package: &Path, bytes: &[u8]) -> Result<String, PackageError> {
    let hash = hash_bytes(bytes);
    let directory = package.join("media").join("objects");
    let owned = Directory::create(package)
        .and_then(|root| root.child("media"))
        .and_then(|media| media.child("objects"))
        .map_err(|error| io_error(&directory, error))?;
    if owned
        .has_file(&hash)
        .map_err(|error| io_error(&directory, error))?
    {
        let existing = owned
            .read(&hash)
            .map_err(|error| io_error(&directory, error))?;
        if existing != bytes {
            return Err(PackageError::new(format!(
                "object {hash} already holds different bytes"
            )));
        }
        return Ok(hash);
    }
    owned
        .write_atomic(&hash, bytes, false)
        .map_err(|error| io_error(&directory, error))?;
    Ok(hash)
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
        if current.contains("/media/objects/") && current.contains("/cache/") {
            return Ok(());
        }
        let has_objects = current.contains("/media/objects/");
        let has_cache = current.contains("/cache/");
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
        return Err(PackageError::new(
            "object hash must be 64 lowercase hex digits",
        ));
    }
    let path = package.join("media").join("objects").join(hash);
    let bytes = Directory::open(package)
        .and_then(|root| {
            root.optional_child("media")?.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "media is missing")
            })
        })
        .and_then(|media| {
            media.optional_child("objects")?.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "objects is missing")
            })
        })
        .and_then(|objects| objects.read(hash))
        .map_err(|error| io_error(&path, error))?;
    if hash_bytes(&bytes) != hash {
        return Err(PackageError::new(format!(
            "object {hash} does not match its BLAKE3"
        )));
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

fn io_error(path: &Path, error: std::io::Error) -> PackageError {
    PackageError::new(format!("{}: {error}", path.display()))
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
