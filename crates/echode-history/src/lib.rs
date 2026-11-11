//! Plain-text projects committed in the background with gix.
//!
//! Product packages store an [`echode_model::Project`] through [`Autosave`].
//! The Phase 0 benchmark is available through `spike` with the opt-in `spike` feature.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

mod autosave;
mod document;
mod error;
mod export;
mod export_settings;
#[cfg(feature = "spike")]
mod format;
mod host;
mod media;
mod objects;
mod package_io;
mod peaks;
#[cfg(feature = "spike")]
mod project;
#[cfg(feature = "spike")]
mod store;
mod thin;
mod transport;

#[cfg(feature = "spike")]
pub mod spike;

pub use autosave::{Autosave, PendingAutosave, SavedRevision};
pub use document::RecoveryReason;
pub use document::{
    AUTOSAVE_REF, PackageError, append_journal, compact_journal, mark_save_pending, read_package,
    recover, write_package,
};
pub use echode_actions::HistoryUsage;
pub use error::{
    AutosaveError, DocumentError, EntryKind, GitOperation, JournalError, MediaError,
    MigrationError, PackageEntryError, PackageKind, PackageValidationError, SettingsError,
    ThinningError,
};
pub use export::freeze_export;
pub use export_settings::{read_export_settings, write_export_settings};
pub use gix::ObjectId;
pub use host::{DocumentHost, ElementError, HostError, RecoveryNotice, ReferenceError};
pub use media::{
    AudioFileReader, DecodedPcm, StoredAudio, decode_audio_file, import_audio, read_pcm,
};
pub use objects::{hash_bytes as object_hash, read_object, write_object};
pub use peaks::{PeakColumn, Waveforms, cache_peaks};
pub use thin::{
    DAY, HOUR, HOURLY_SPAN, RECENT, SavePoint, StoreSize, git_store_size, kept_indexes,
    pack_and_thin, seed_autosaves, tip_text, write_bundle,
};
pub use transport::{TransportSettings, read_transport_settings, write_transport_settings};
