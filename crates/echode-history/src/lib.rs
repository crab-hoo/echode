//! Plain-text projects committed in the background with gix.
//!
//! Product packages store an [`echode_model::Project`] through [`Autosave`].
//! The 1,000-track benchmark lives in [`spike`].

#![cfg_attr(not(target_os = "macos"), allow(unused))]

mod autosave;
mod document;
mod format;
mod host;
mod media;
mod objects;
mod package_io;
mod peaks;
mod project;
mod store;
mod thin;

pub mod spike;

pub use autosave::{Autosave, PendingAutosave, SavedRevision};
pub use document::{
    AUTOSAVE_REF, PackageError, append_journal, compact_journal, mark_save_pending, read_package,
    recover, write_package,
};
pub use gix::ObjectId;
pub use host::{DocumentHost, HostError};
pub use media::{
    AudioFileReader, DecodedPcm, StoredAudio, decode_audio_file, import_audio, read_pcm,
};
pub use objects::{hash_bytes as object_hash, read_object, write_object};
pub use peaks::{PeakColumn, Waveforms, cache_peaks};
pub use thin::{
    DAY, HOUR, HOURLY_SPAN, RECENT, SavePoint, StoreSize, git_store_size, kept_indexes,
    pack_and_thin, seed_autosaves, tip_text, write_bundle,
};
