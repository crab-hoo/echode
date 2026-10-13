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
//! - A content does not list its own id as a take.
//! - A note has a pitch in 0..=127, a positive length, no articulation, and
//!   `start + length` does not overflow. Velocity and release may be any
//!   `u16`. A negative start is allowed.
//! - Expression curves are empty.
//! - Notes in a content are sorted by start, then pitch, then id.
//! - Each track ends with one mixer. CLAP devices, if any, sit before it in
//!   insertion order and carry a plugin id with no parameters. The mixer's
//!   parameters are gain, pan, mute, and solo, in that order, with that
//!   control's minimum, maximum, and default. The value lies in range.
//! - Placements on a track are stored in start order. Their half-open ranges
//!   do not overlap. Each has a positive length, a finite gain of at least
//!   zero, and fades that fit in a frame count. Its content exists, and its
//!   origin does not overflow.
//! - A content has at most one auto placement, and that placement spans
//!   exactly the bars of its sounding notes.
//! - An audio reference names a BLAKE3-256 hash and a mono or stereo file with a
//!   positive rate and length.
//! - Every content has at least one placement.
//! - Parents form a forest: every parent exists, and no track is its own ancestor.
//! - The track array is the arrange order. Sibling order keys are unique
//!   fractional indexes.

mod changeset;
mod device;
mod id;
mod invariants;
mod model;
mod placement;
mod time;

pub use changeset::{Change, ChangeSet, Field, ObjectKey, ObjectKind, Property, Value};
pub use device::{
    Device, DeviceKind, GAIN_MAX, MixerIds, ParamKey, Parameter, test_mixer, valid_plugin_id,
};
pub use id::Id;
pub use invariants::InvariantError;
pub use model::{
    AudioRef, ClipKind, Content, DEFAULT_FADE_FLICKS, Expression, ExpressionKind, ExpressionPoint,
    FADE_RATE_LIMIT, Lane, ModelError, Note, NoteDraft, NoteGeometry, Placement, Project, Track,
    fade_allowed, fade_frames, initial_order, is_media_hash, midi7_to_velocity, velocity_to_midi7,
};
pub use time::{
    FLICKS_PER_SECOND, Flick, Meter, TICKS_PER_QUARTER, Tempo, TempoMap, TempoPoint, Tick,
};
