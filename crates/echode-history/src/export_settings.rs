//! Portable GUI export preferences, outside the project model and its history.
//!
//! The GUI owns the versioned JSON schema. This layer only accepts bounded JSON
//! objects and uses the same no-follow, atomic package I/O as other entries.

use std::io::Read;
use std::path::Path;

use crate::document::{PackageError, io_error, open_package};

const FILE: &str = "export.json";
// This bounds a preference record only, not a song, track, or audio buffer.
const MAX_BYTES: usize = 16_384;

fn validate(text: &str) -> Result<(), PackageError> {
    if text.len() > MAX_BYTES {
        return Err(PackageError::new("export settings exceed 16 KiB"));
    }
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text)
        .map(|_| ())
        .map_err(|error| PackageError::new(format!("invalid export settings: {error}")))
}

/// Missing, unreadable, oversized or malformed preferences have no value.
/// Reads never repair the package or follow a symbolic link.
pub fn read_export_settings(package: &Path) -> Option<String> {
    let mut text = String::new();
    open_package(package)
        .ok()?
        .open_file(FILE, false, false)
        .ok()?
        .take((MAX_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    validate(&text).ok()?;
    Some(text)
}

/// The owning document worker calls this after checking recovery consent.
/// Preferences are atomically replaced but not synced or included in Undo.
pub fn write_export_settings(package: &Path, text: &str) -> Result<(), PackageError> {
    validate(text)?;
    open_package(package)?
        .write_atomic(FILE, text.as_bytes(), false)
        .map_err(|error| io_error(&package.join(FILE), error))
}
