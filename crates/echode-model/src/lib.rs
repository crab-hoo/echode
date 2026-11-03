//! Document model shared by the UI, the engine, and history.
//!
//! A project is an immutable snapshot. An edit returns a new project and keeps
//! unchanged tracks behind the same `Arc`. This crate allocates, so the audio
//! thread reads a snapshot that was published earlier instead of calling it.
//!
//! # Invariants
//!
//! [`Project::check_invariants`] reports the first rule a snapshot breaks.
//!
//! - The meter divides into a whole number of ticks per bar.
//! - Content, note, track, device, parameter, and placement ids are unique.
//! - Contents are sorted by id.
//! - A content does not list its own id as a take.
//! - A note has a pitch in 0..=127, a positive length, no articulation, and
//!   `start + length` does not overflow. Velocity and release may be any
//!   `u16`. A negative start is allowed.
//! - Expression curves are empty.
//! - Notes in a content are sorted by start, then pitch, then id.
//! - Each track ends with one mixer. CLAP devices, if any, sit before it in
//!   insertion order and carry a plugin id with no parameters. The mixer is
//!   never bypassed or offline and holds no plugin state. Its
//!   parameters are gain, pan, mute, and solo, in that order, with that
//!   control's minimum, maximum, and default. The value lies in range.
//! - Placements on a track are stored in start order. Their half-open ranges
//!   do not overlap. Each has a positive length, a finite gain from zero
//!   through [`CLIP_GAIN_MAX`], and fades that fit in a frame count. Its
//!   content exists, and its origin (`start - offset`) does not overflow.
//!   A negative offset is silence before the media, not an invalid clip.
//! - A content has at most one auto placement, and that placement spans
//!   exactly the bars of its notes counted from its content origin.
//! - An audio reference names a BLAKE3-256 hash and a mono or stereo file with a
//!   positive rate and length.
//! - Every content has at least one placement.
//! - Parents form a forest: every parent exists, and no track is its own ancestor.
//! - The track array is the arrange order. Sibling order keys are unique
//!   fractional indexes.

mod change_values;
mod changeset;
mod clip_move;
mod content_fields;
mod device;
mod device_fields;
mod duplicate;
mod id;
mod invariants;
mod model;
#[cfg(test)]
mod model_scale_tests;
mod note_fields;
mod note_velocity;
mod package_fields;
mod parameter_fields;
mod placement;
mod placement_fields;
mod project_fields;
mod tempo_fields;
mod time;
mod track_fields;

pub use changeset::{Change, ChangeSet, Field, ObjectKey, ObjectKind, Property, Value};
pub use device::{
    Device, DeviceKind, GAIN_MAX, MixerIds, PAN_MAX, PAN_MIN, ParamKey, Parameter, test_mixer,
    valid_plugin_id,
};
pub use id::Id;
pub use invariants::InvariantError;
pub use model::{
    AudioRef, CLIP_GAIN_MAX, ClipKind, Content, ContentMetadata, DEFAULT_FADE_FLICKS, Expression,
    ExpressionKind, ExpressionPoint, FADE_RATE_LIMIT, Lane, MIDI7_MAX, ModelError, Note, NoteDraft,
    NoteGeometry, Placement, Project, ProjectMetadata, Track, TrackMetadata, clip_gain_allowed,
    fade_allowed, fade_frames, initial_order, is_media_hash, midi7_to_velocity, velocity_to_midi7,
};
pub use time::{
    FLICKS_PER_SECOND, Flick, Meter, TICKS_PER_QUARTER, Tempo, TempoMap, TempoPoint, Tick,
    media_sample_at, placement_media_lead, tick_to_sample,
};
