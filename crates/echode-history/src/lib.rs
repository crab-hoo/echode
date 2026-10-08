//! Plain-text projects committed in the background with gix.
//!
//! [`Project`] is the benchmark document used by the history spike.
//! Product packages store an [`echode_model::Project`] through [`Autosave`].

#![cfg_attr(not(target_os = "macos"), allow(unused))]

mod autosave;
mod document;
mod format;
mod project;
mod store;

pub use autosave::{Autosave, PendingAutosave};
pub use document::{
    AUTOSAVE_REF, JournalBase, PackageError, read_package, recover, write_journal, write_package,
};
pub use gix::ObjectId;
pub use project::{Project, SPIKE_NOTES_PER_TRACK, SPIKE_TRACKS, Track};
pub use store::{History, PendingSave};
