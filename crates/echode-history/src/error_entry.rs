//! Why a package entry was refused before a read or write reached it (M1).
//!
//! The package is opened relative to retained descriptors and never follows a
//! link below its root (PRO-01). A refusal is the inner error of an
//! `io::Error`, so its kind and its shown sentence stay those of the
//! constant text it replaces, while callers read the name, kind and link
//! count from `io::Error::get_ref`.

use std::ffi::OsString;
use std::fmt;

/// The file type of a package entry, from its mode's format bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    SymbolicLink,
    /// A FIFO, socket or device. The value is the mode's format bits.
    Other(u32),
}

impl EntryKind {
    /// The kind named by `mode`'s format bits (`S_IFMT`); other bits are ignored.
    pub(crate) fn from_mode(mode: u32) -> Self {
        let format = mode & u32::from(libc::S_IFMT);
        if format == u32::from(libc::S_IFREG) {
            Self::File
        } else if format == u32::from(libc::S_IFDIR) {
            Self::Directory
        } else if format == u32::from(libc::S_IFLNK) {
            Self::SymbolicLink
        } else {
            Self::Other(format)
        }
    }
}

/// A package entry that the retained-descriptor access refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageEntryError {
    /// A folder the read needs is absent. The `io::Error` kind is `NotFound`.
    MissingFolder { name: OsString },
    /// The name is not one normal path component, such as `a/b` or `..`.
    NotSingleComponent { name: OsString },
    /// The entry exists with another type, a symbolic link included.
    WrongType {
        name: OsString,
        actual: EntryKind,
        expected: EntryKind,
    },
    /// The opened file is not a regular file, or a file opened for writing
    /// has other hard links, so a write could reach a file outside the package.
    NotRegularUnsharedFile { kind: EntryKind, links: u64 },
}

impl PackageEntryError {
    /// The `io::Error` kind each refusal had as constant text.
    pub(crate) fn kind(&self) -> std::io::ErrorKind {
        match self {
            Self::MissingFolder { .. } => std::io::ErrorKind::NotFound,
            Self::NotSingleComponent { .. }
            | Self::WrongType { .. }
            | Self::NotRegularUnsharedFile { .. } => std::io::ErrorKind::InvalidInput,
        }
    }
}

impl fmt::Display for PackageEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFolder { name } => write!(f, "{} is missing", name.to_string_lossy()),
            Self::NotSingleComponent { .. } => {
                f.write_str("package entry must be a single path component")
            }
            Self::WrongType { name, actual, .. } => {
                let reason = if *actual == EntryKind::SymbolicLink {
                    "is a symbolic link"
                } else {
                    "has the wrong file type"
                };
                write!(f, "{} {reason}", name.to_string_lossy())
            }
            Self::NotRegularUnsharedFile { .. } => {
                f.write_str("package file is not a regular unshared file")
            }
        }
    }
}

impl std::error::Error for PackageEntryError {}

impl From<PackageEntryError> for std::io::Error {
    fn from(error: PackageEntryError) -> Self {
        Self::new(error.kind(), error)
    }
}
