//! Typed causes for snapshot validation and derived track order.

use std::fmt;

use crate::{Id, model::CLIP_GAIN_MAX};

/// Why track keys cannot produce the required arrange order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArrangeError {
    InvalidKey {
        track: Id,
        name: String,
    },
    DuplicateKey {
        first: Id,
        first_name: String,
        second: Id,
        second_name: String,
    },
    Cycle {
        track: Id,
        name: String,
    },
    MissingTrack {
        track: Id,
        name: String,
    },
    OutOfOrder {
        track: Id,
        name: String,
    },
}

impl fmt::Display for ArrangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey { name, .. } => write!(
                f,
                "track {name} has an order key that is not a fractional index"
            ),
            Self::DuplicateKey {
                first_name,
                second_name,
                ..
            } => write!(
                f,
                "tracks {first_name} and {second_name} share an order key"
            ),
            Self::Cycle { name, .. } => write!(f, "track {name} is its own ancestor"),
            Self::MissingTrack { name, .. } => {
                write!(f, "track {name} is not in the arrange order")
            }
            Self::OutOfOrder { name, .. } => write!(f, "track {name} is out of arrange order"),
        }
    }
}

impl std::error::Error for ArrangeError {}

/// The first rule a snapshot broke, with the objects needed to diagnose it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvariantError {
    InvalidMeter,
    ContentOrder,
    DuplicateId {
        id: Id,
    },
    SelfTake {
        content: Id,
    },
    InvalidNote {
        note: Id,
    },
    NoteEnd {
        note: Id,
    },
    Expressions {
        content: Id,
    },
    NoteOrder {
        content: Id,
        first: Id,
        second: Id,
    },
    MissingMixer {
        track: Id,
        name: String,
    },
    InvalidMixer {
        track: Id,
        name: String,
    },
    InvalidPlugin {
        track: Id,
        name: String,
    },
    MixerParameters {
        track: Id,
        name: String,
    },
    ParameterRange {
        track: Id,
        track_name: String,
        parameter: Id,
    },
    ClipEnd {
        clip: Id,
    },
    MissingContent {
        clip: Id,
    },
    ClipOrigin {
        clip: Id,
    },
    EmptyClip {
        clip: Id,
    },
    ClipGain {
        clip: Id,
    },
    ClipFade {
        clip: Id,
    },
    ClipOverlap {
        clip: Id,
        track: Id,
        track_name: String,
    },
    NoteStart {
        note: Id,
    },
    EmptyAuto {
        clip: Id,
    },
    AutoStart {
        clip: Id,
    },
    AutoEnd {
        clip: Id,
    },
    AutoBounds {
        clip: Id,
        start: i64,
        end: i64,
        low: i64,
        high: i64,
    },
    SharedAutoContent {
        content: Id,
    },
    UnplacedContent {
        content: Id,
    },
    InvalidAudio {
        content: Id,
    },
    MissingParent {
        track: Id,
        parent: Id,
    },
    TrackCycle {
        track: Id,
    },
    Order(Box<ArrangeError>),
}

impl fmt::Display for InvariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMeter => f.write_str("the meter has no whole-tick bar"),
            Self::ContentOrder => f.write_str("contents are out of content id order"),
            Self::DuplicateId { id } => write!(f, "id {id} is used twice"),
            Self::SelfTake { content } => write!(f, "content {content} lists itself as a take"),
            Self::InvalidNote { note } => write!(f, "note {note} is not usable"),
            Self::NoteEnd { note } => write!(f, "note {note} ends past the time range"),
            Self::Expressions { content } => write!(
                f,
                "content {content} has expression curves before they can be edited"
            ),
            Self::NoteOrder {
                content,
                first,
                second,
            } => write!(
                f,
                "notes {first} then {second} in content {content} are out of start, pitch, id order"
            ),
            Self::MissingMixer { name, .. } => write!(f, "track {name} does not have one mixer"),
            Self::InvalidMixer { name, .. } => write!(
                f,
                "track {name} mixer is bypassed, offline or holds plugin state"
            ),
            Self::InvalidPlugin { name, .. } => {
                write!(f, "track {name} has a plugin device without a CLAP id")
            }
            Self::MixerParameters { name, .. } => {
                write!(f, "track {name} mixer does not have its four parameters")
            }
            Self::ParameterRange {
                parameter,
                track_name,
                ..
            } => write!(
                f,
                "parameter {parameter} on {track_name} is outside its range"
            ),
            Self::ClipEnd { clip } => write!(f, "clip {clip} ends past the time range"),
            Self::MissingContent { clip } => write!(f, "clip {clip} names a missing content"),
            Self::ClipOrigin { clip } => write!(f, "clip {clip} has no origin"),
            Self::EmptyClip { clip } => write!(f, "clip {clip} is empty"),
            Self::ClipGain { clip } => write!(
                f,
                "clip {clip} gain must be finite, at least zero, and at most {CLIP_GAIN_MAX}"
            ),
            Self::ClipFade { clip } => {
                write!(
                    f,
                    "clip {clip} fade is negative or does not fit in a frame count"
                )
            }
            Self::ClipOverlap {
                clip, track_name, ..
            } => write!(
                f,
                "clip {clip} starts before the previous clip on {track_name} ends"
            ),
            Self::NoteStart { note } => write!(f, "note {note} starts past the time range"),
            Self::EmptyAuto { clip } => write!(f, "auto clip {clip} has no notes"),
            Self::AutoStart { clip } => write!(f, "auto clip {clip} starts past the time range"),
            Self::AutoEnd { clip } => write!(f, "auto clip {clip} ends past the time range"),
            Self::AutoBounds {
                clip,
                start,
                end,
                low,
                high,
            } => write!(
                f,
                "auto clip {clip} spans {start}..{end}, but its notes cover bars {low}..={high}"
            ),
            Self::SharedAutoContent { .. } => {
                f.write_str("a content has more than one auto placement")
            }
            Self::UnplacedContent { content } => write!(f, "content {content} has no placement"),
            Self::InvalidAudio { content } => {
                write!(f, "content {content} has an unusable audio reference")
            }
            Self::MissingParent { track, parent } => {
                write!(f, "track {track} names a missing parent {parent}")
            }
            Self::TrackCycle { track } => write!(f, "track {track} is its own ancestor"),
            Self::Order(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for InvariantError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Order(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}
