//! Keep a render's output outside the package it reads, before recovery or I/O.
use std::path::{Path, PathBuf};

use crate::{ArgumentError, CliError, CliIoOperation};

pub(crate) struct RenderPaths {
    pub(crate) package: PathBuf,
    pub(crate) output: PathBuf,
}

impl RenderPaths {
    pub(crate) fn resolve(package: &Path, output: &Path) -> Result<Self, CliError> {
        let package = package.canonicalize().map_err(|error| {
            CliError::io(CliIoOperation::ResolvePackage, package.to_path_buf(), error)
        })?;
        let name = output.file_name().ok_or(ArgumentError::OutputFileName)?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent.canonicalize().map_err(|error| {
            CliError::io(
                CliIoOperation::ResolveOutputDirectory,
                parent.to_path_buf(),
                error,
            )
        })?;
        let output = parent.join(name);
        // Component comparison, not a string prefix: Song.echode-exports is outside.
        // Also reject an existing leaf alias conservatively, although rename itself
        // replaces a symlink rather than following it.
        let inside = output.starts_with(&package)
            || output
                .canonicalize()
                .is_ok_and(|target| target.starts_with(&package));
        if inside {
            return Err(ArgumentError::OutputInsidePackage.into());
        }
        // Use the resolved parent for both scratch and publication, so retargeting
        // the user's directory symlink during a render cannot redirect the export.
        Ok(Self { package, output })
    }
}

#[cfg(test)]
mod tests {
    use super::RenderPaths;

    #[test]
    fn resolved_output_is_not_retargeted_by_a_directory_alias_change() {
        let root = std::env::temp_dir().join(format!(
            "echode-destination-{}",
            echode_model::Id::generate()
        ));
        let package = root.join("Song.echode");
        let outside = root.join("exports");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        let paths = RenderPaths::resolve(&package, &alias.join("mix.wav")).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&package, &alias).unwrap();
        assert_eq!(
            paths.output,
            outside.canonicalize().unwrap().join("mix.wav")
        );
        std::fs::write(&paths.output, b"export").unwrap();
        assert!(!package.join("mix.wav").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
