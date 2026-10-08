//! Content-addressed bytes for audio and plugin state.
//!
//! A file in `media/objects/` is named by the BLAKE3-256 of its bytes. Git
//! commits do not include that directory.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::document::PackageError;

const GITIGNORE: &str = "/media/objects/\n/cache/\n";

/// Lowercase BLAKE3-256 hex, the only spelling the package accepts.
pub(crate) fn is_content_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// BLAKE3-256 of `bytes`, as 64 lowercase hex digits.
///
/// Plugin hosting will call this when a device can store state.
#[allow(dead_code)]
pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Writes the ignore rules for the object store. Existing rules are kept.
pub(crate) fn ensure_store_gitignore(directory: &Path) -> Result<(), PackageError> {
    fs::create_dir_all(directory).map_err(|error| io_error(directory, error))?;
    let path = directory.join(".gitignore");
    if path.exists() {
        let current = fs::read_to_string(&path).map_err(|error| io_error(&path, error))?;
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
        fs::write(&path, next).map_err(|error| io_error(&path, error))?;
        return Ok(());
    }
    fs::write(&path, GITIGNORE).map_err(|error| io_error(&path, error))
}

/// Reads `media/objects/<hash>` after checking the name, the link, and the bytes.
///
/// Plugin hosting will call this when a device can store state.
#[allow(dead_code)]
pub(crate) fn read_object(package: &Path, hash: &str) -> Result<Vec<u8>, PackageError> {
    if !is_content_hash(hash) {
        return Err(PackageError::new(
            "state hash must be 64 lowercase hex digits",
        ));
    }
    let path = package.join("media").join("objects").join(hash);
    let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(PackageError::new(format!(
            "{} is a symbolic link",
            path.display()
        )));
    }
    let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
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
