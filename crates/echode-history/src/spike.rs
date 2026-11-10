//! The Phase 0 history benchmark.
//!
//! Product packages use [`crate::Autosave`] and [`echode_model::Project`].

pub use crate::error::SpikeError;
pub use crate::project::{Project, SPIKE_NOTES_PER_TRACK, SPIKE_TRACKS, Track};
pub use crate::store::{History, PendingSave};
