//! Playback settings kept beside the project, outside history and Undo.
//!
//! `transport.json` holds the remembered playback start (ADR 0155). Autosave
//! builds its commit tree from the model alone, so this file is never part of
//! a revision, and restoring one does not move the start.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::document::{PackageError, io_error, open_package};

const FILE: &str = "transport.json";
const VERSION: u32 = 1;

/// Playback state that survives reopening. It is never journaled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportSettings {
    /// The remembered playback start in project ticks. Never negative.
    pub start: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    start: i64,
}

/// The package's settings. A missing, unreadable or newer file gives the
/// defaults: they are a convenience, and the next change replaces the file.
pub fn read_transport_settings(package: &Path) -> TransportSettings {
    let Some(text) = open_package(package)
        .ok()
        .and_then(|root| root.read_text(FILE).ok())
    else {
        return TransportSettings::default();
    };
    match serde_json::from_str::<Stored>(&text) {
        Ok(Stored {
            version: VERSION,
            start,
        }) if start >= 0 => TransportSettings { start },
        _ => TransportSettings::default(),
    }
}

/// Replaces the file atomically. It is not synced: a crash keeps the previous
/// or the new start, never a partial file.
pub fn write_transport_settings(
    package: &Path,
    settings: TransportSettings,
) -> Result<(), PackageError> {
    if settings.start < 0 {
        return Err(PackageError::new("playback start must not be negative"));
    }
    let root = open_package(package)?;
    let mut text = serde_json::to_string(&Stored {
        version: VERSION,
        start: settings.start,
    })
    .map_err(|error| PackageError::new(error.to_string()))?;
    text.push('\n');
    root.write_atomic(FILE, text.as_bytes(), false)
        .map_err(|error| io_error(&package.join(FILE), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_survives_reopening_and_bad_files_fall_back_to_zero() {
        let directory =
            std::env::temp_dir().join(format!("echode-transport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let package = directory.join("song.echode");
        crate::write_package(&package, &echode_model::Project::new()).unwrap();
        assert_eq!(read_transport_settings(&package).start, 0);
        write_transport_settings(&package, TransportSettings { start: 705_600 }).unwrap();
        assert_eq!(read_transport_settings(&package).start, 705_600);
        assert!(write_transport_settings(&package, TransportSettings { start: -1 }).is_err());
        assert_eq!(read_transport_settings(&package).start, 705_600);
        for text in [
            "not json",
            "{\"version\":2,\"start\":5}",
            "{\"version\":1,\"start\":-5}",
            "{\"version\":1,\"start\":5,\"loop\":[0,1]}",
        ] {
            std::fs::write(package.join(FILE), text).unwrap();
            assert_eq!(read_transport_settings(&package).start, 0, "{text}");
        }
        // A link is never followed out of the package.
        std::fs::remove_file(package.join(FILE)).unwrap();
        let outside = directory.join("outside.json");
        std::fs::write(&outside, "{\"version\":1,\"start\":9}").unwrap();
        std::os::unix::fs::symlink(&outside, package.join(FILE)).unwrap();
        assert_eq!(read_transport_settings(&package).start, 0);
        assert!(write_transport_settings(&package, TransportSettings { start: 1 }).is_err());
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "{\"version\":1,\"start\":9}"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
