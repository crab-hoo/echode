//! Document model shared by the UI, the engine, and history.
//!
//! A project is an immutable snapshot. An edit returns a new project and keeps
//! unchanged tracks behind the same `Arc`. This crate allocates, so the audio
//! thread reads a snapshot that was published earlier instead of calling it.

mod id;
mod invariants;
mod model;
mod time;

pub use id::Id;
pub use invariants::InvariantError;
pub use model::{Clip, ClipKind, ModelError, Note, NoteDraft, Project, Track};
pub use time::{
    FLICKS_PER_SECOND, Flick, Meter, TICKS_PER_QUARTER, Tempo, TempoMap, TempoPoint, Tick,
};
