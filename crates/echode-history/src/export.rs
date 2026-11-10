//! Private, self-contained packages for exports of a recorded snapshot.

use std::collections::BTreeSet;
use std::path::Path;

use echode_model::Project;

use crate::{PackageError, read_object, write_object, write_package};

/// Creates a new package, never overwriting an existing path. The caller owns
/// its lifetime. No journal, Git history, cache, or unrelated media is copied.
/// Capture the immutable project on the document worker before calling this.
pub fn freeze_export(
    source: &Path,
    project: &Project,
    destination: &Path,
) -> Result<(), PackageError> {
    project
        .check_invariants()
        .map_err(PackageError::ExportSnapshot)?;
    std::fs::create_dir(destination).map_err(PackageError::from)?;
    let result = (|| {
        let hashes: BTreeSet<&str> = project
            .contents()
            .iter()
            .filter_map(|content| content.audio.as_ref().map(|audio| audio.hash.as_str()))
            .chain(
                project
                    .tracks()
                    .iter()
                    .flat_map(|track| &track.devices)
                    .filter_map(|device| device.state.as_deref()),
            )
            .collect();
        for hash in hashes {
            let bytes = read_object(source, hash)?;
            write_object(destination, &bytes)?;
        }
        write_package(destination, project)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}
