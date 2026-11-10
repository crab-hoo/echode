//! Semantic refusals and text decoding failures in history thinning.

use std::fmt;

#[derive(Debug)]
pub enum ThinningError {
    NothingToKeep,
    NoReferenceTips,
    SymbolicAutosave,
    MissingProject,
    Text(std::string::FromUtf8Error),
}

impl fmt::Display for ThinningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingToKeep => f.write_str("nothing to keep"),
            Self::NoReferenceTips => f.write_str("the repository has no commit to keep"),
            Self::SymbolicAutosave => f.write_str("autosave ref is not a commit"),
            Self::MissingProject => f.write_str("project.json is missing"),
            Self::Text(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ThinningError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Text(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ThinningError> for super::PackageError {
    fn from(error: ThinningError) -> Self {
        Self::Thinning(error)
    }
}
