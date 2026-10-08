//! Plain-text projects committed in the background with gix.
//!
//! Product packages store an [`echode_model::Project`] through [`Autosave`].
//! The 1,000-track benchmark lives in [`spike`].

#![cfg_attr(not(target_os = "macos"), allow(unused))]

mod autosave;
mod document;
mod format;
mod project;
mod store;

pub mod spike;

pub use autosave::{Autosave, PendingAutosave};
pub use document::{
    AUTOSAVE_REF, JournalBase, PackageError, read_package, recover, write_journal, write_package,
};
pub use gix::ObjectId;
