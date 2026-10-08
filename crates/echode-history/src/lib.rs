//! Plain-text projects committed in the background with gix.
//!
//! Product packages store an [`echode_model::Project`] through [`Autosave`].
//! The 1,000-track benchmark lives in [`spike`].

#![cfg_attr(not(target_os = "macos"), allow(unused))]

mod autosave;
mod document;
mod format;
mod host;
mod objects;
mod project;
mod store;

pub mod spike;

pub use autosave::{Autosave, PendingAutosave};
pub use document::{
    AUTOSAVE_REF, PackageError, append_journal, compact_journal, mark_save_pending, read_package,
    recover, write_package,
};
pub use gix::ObjectId;
pub use host::{DocumentHost, HostError};
