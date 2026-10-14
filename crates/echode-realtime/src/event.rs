//! One block's events, in CLAP's shape.
//!
//! `None` in a destination is CLAP's -1. Assigned note numbers are `u32` in
//! `0..i32::MAX` and are never used as that sentinel. Transport has no track.

use std::cmp::Ordering;

/// Where a note or parameter event applies. `None` matches any value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub port: Option<u16>,
    pub channel: Option<u16>,
    pub key: Option<u8>,
    pub note_id: Option<u32>,
}

impl Target {
    /// A note the engine started: port 0, channel 0, a key, and a note number.
    pub const fn note(note_id: u32, key: u8) -> Self {
        Self {
            port: Some(0),
            channel: Some(0),
            key: Some(key),
            note_id: Some(note_id),
        }
    }

    /// No port, channel, key, or note. CLAP receives -1 for each.
    pub const fn any() -> Self {
        Self {
            port: None,
            channel: None,
            key: None,
            note_id: None,
        }
    }
}

/// Note expression carried in the block. The value is already in CLAP units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpressionKind {
    Pitch,
    Timbre,
    Pressure,
}

/// Host transport for one sample in the block. Flags are booleans, not CLAP bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    pub time: u32,
    pub playing: bool,
    pub recording: bool,
    pub loop_active: bool,
    pub tempo_bpm: f64,
    pub tempo_inc: f64,
    pub song_pos_beats: f64,
    pub song_pos_seconds: f64,
    pub loop_start_beats: f64,
    pub loop_end_beats: f64,
    pub loop_start_seconds: f64,
    pub loop_end_seconds: f64,
    pub bar_start: f64,
    pub bar_number: i32,
    pub signature_numerator: u16,
    pub signature_denominator: u16,
}

/// A note-on or note-off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    pub time: u32,
    pub track: u32,
    pub target: Target,
    pub velocity: f64,
}

/// A choke. It has no velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChokeEvent {
    pub time: u32,
    pub track: u32,
    pub target: Target,
}

/// A note expression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpressionEvent {
    pub time: u32,
    pub track: u32,
    pub target: Target,
    pub kind: ExpressionKind,
    pub value: f64,
}

/// A parameter value or modulation amount.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamEvent {
    pub time: u32,
    pub track: u32,
    pub param_id: u32,
    pub target: Target,
    pub value: f64,
}

/// One sample-accurate event. The audio thread only writes into a caller buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Transport(Transport),
    NoteOn(NoteEvent),
    NoteOff(NoteEvent),
    NoteChoke(ChokeEvent),
    Expression(ExpressionEvent),
    ParamValue(ParamEvent),
    ParamMod(ParamEvent),
}

impl Event {
    pub const fn note_on(time: u32, track: u32, note_id: u32, key: u8, velocity: f64) -> Self {
        Self::NoteOn(NoteEvent {
            time,
            track,
            target: Target::note(note_id, key),
            velocity,
        })
    }

    pub const fn note_off(time: u32, track: u32, note_id: u32, key: u8) -> Self {
        Self::NoteOff(NoteEvent {
            time,
            track,
            target: Target::note(note_id, key),
            velocity: 0.0,
        })
    }

    pub const fn time(self) -> u32 {
        match self {
            Self::Transport(event) => event.time,
            Self::NoteOn(event) | Self::NoteOff(event) => event.time,
            Self::NoteChoke(event) => event.time,
            Self::Expression(event) => event.time,
            Self::ParamValue(event) | Self::ParamMod(event) => event.time,
        }
    }

    /// `None` for transport, which is not dispatched to one track.
    pub const fn track_key(self) -> Option<u32> {
        match self {
            Self::Transport(_) => None,
            Self::NoteOn(event) | Self::NoteOff(event) => Some(event.track),
            Self::NoteChoke(event) => Some(event.track),
            Self::Expression(event) => Some(event.track),
            Self::ParamValue(event) | Self::ParamMod(event) => Some(event.track),
        }
    }

    pub const fn is_note_on(self) -> bool {
        matches!(self, Self::NoteOn(_))
    }

    pub const fn is_note_off(self) -> bool {
        matches!(self, Self::NoteOff(_))
    }

    pub fn key(self) -> Option<u8> {
        self.target().and_then(|target| target.key)
    }

    pub fn note_id(self) -> Option<u32> {
        self.target().and_then(|target| target.note_id)
    }

    pub const fn velocity(self) -> f64 {
        match self {
            Self::NoteOn(event) | Self::NoteOff(event) => event.velocity,
            _ => 0.0,
        }
    }

    fn target(self) -> Option<Target> {
        match self {
            Self::Transport(_) => None,
            Self::NoteOn(event) | Self::NoteOff(event) => Some(event.target),
            Self::NoteChoke(event) => Some(event.target),
            Self::Expression(event) => Some(event.target),
            Self::ParamValue(event) | Self::ParamMod(event) => Some(event.target),
        }
    }
}

/// Total order for one block.
///
/// Same-sample rank is transport, note-off, choke, note-on, expression,
/// parameter value, modulation. Absent destination fields sort before present
/// ones. Expression kind and parameter id break the remaining ties. Equal keys
/// keep the order they were written.
///
/// `order` and `scratch` are workspace the caller allocated before the audio
/// thread. Both must be at least as long as `events`. The sort is the standard
/// library's in-place ordering, with the original index as the last key, so
/// equal musical keys stay in the order they were written and this function
/// does not allocate. Returns false without moving `events` when the workspace
/// is short.
#[must_use]
pub fn sort_events(events: &mut [Event], order: &mut [usize], scratch: &mut [Event]) -> bool {
    let count = events.len();
    if order.len() < count || scratch.len() < count {
        return false;
    }
    let order = &mut order[..count];
    for (index, slot) in order.iter_mut().enumerate() {
        *slot = index;
    }
    order.sort_unstable_by(|&left, &right| {
        cmp_event(&events[left], &events[right]).then(left.cmp(&right))
    });
    let scratch = &mut scratch[..count];
    for (slot, source) in scratch.iter_mut().zip(order.iter().copied()) {
        *slot = events[source];
    }
    events.copy_from_slice(scratch);
    true
}

/// Musical order used by [`sort_events`]. Equal keys stay in write order.
pub fn event_cmp(left: &Event, right: &Event) -> Ordering {
    cmp_event(left, right)
}

fn cmp_event(left: &Event, right: &Event) -> Ordering {
    left.time()
        .cmp(&right.time())
        .then(rank(left).cmp(&rank(right)))
        .then(track_of(left).cmp(&track_of(right)))
        .then(cmp_option(port_of(left), port_of(right)))
        .then(cmp_option(channel_of(left), channel_of(right)))
        .then(cmp_option(left.key(), right.key()))
        .then(cmp_option(left.note_id(), right.note_id()))
        .then(extra(left).cmp(&extra(right)))
}

fn rank(event: &Event) -> u8 {
    match event {
        Event::Transport(_) => 0,
        Event::NoteOff(_) => 1,
        Event::NoteChoke(_) => 2,
        Event::NoteOn(_) => 3,
        Event::Expression(_) => 4,
        Event::ParamValue(_) => 5,
        Event::ParamMod(_) => 6,
    }
}

fn track_of(event: &Event) -> Option<u32> {
    event.track_key()
}

fn port_of(event: &Event) -> Option<u16> {
    event.target().and_then(|target| target.port)
}

fn channel_of(event: &Event) -> Option<u16> {
    event.target().and_then(|target| target.channel)
}

/// Expression kind, then parameter id. Notes and transport have neither.
fn extra(event: &Event) -> (u8, u32) {
    match event {
        Event::Expression(event) => (event.kind as u8, 0),
        Event::ParamValue(event) | Event::ParamMod(event) => (0, event.param_id),
        _ => (0, 0),
    }
}

fn cmp_option<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => left.cmp(&right),
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, ExpressionEvent, ExpressionKind, ParamEvent, Target, sort_events};

    fn sort_slice(events: &mut [Event]) -> bool {
        let mut order = vec![0; events.len()];
        let mut scratch = vec![Event::note_off(0, 0, 0, 0); events.len()];
        sort_events(events, &mut order, &mut scratch)
    }

    fn param(param_id: u32, value: f64) -> Event {
        Event::ParamValue(ParamEvent {
            time: 0,
            track: 1,
            param_id,
            target: Target::any(),
            value,
        })
    }

    #[test]
    fn the_same_sample_orders_params_and_expressions_and_keeps_ties() {
        let mut events = [
            param(2, 0.2),
            Event::Expression(ExpressionEvent {
                time: 0,
                track: 1,
                target: Target::note(4, 60),
                kind: ExpressionKind::Pressure,
                value: 0.2,
            }),
            param(1, 0.5),
            Event::Expression(ExpressionEvent {
                time: 0,
                track: 1,
                target: Target::note(4, 60),
                kind: ExpressionKind::Pitch,
                value: 1.0,
            }),
            param(2, 0.9),
            Event::note_on(0, 1, 4, 60, 1.0),
        ];
        assert!(sort_slice(&mut events));
        assert!(events[0].is_note_on());
        assert!(matches!(
            events[1],
            Event::Expression(ExpressionEvent {
                kind: ExpressionKind::Pitch,
                ..
            })
        ));
        assert!(matches!(
            events[2],
            Event::Expression(ExpressionEvent {
                kind: ExpressionKind::Pressure,
                ..
            })
        ));
        assert!(matches!(
            events[3],
            Event::ParamValue(ParamEvent { param_id: 1, .. })
        ));
        assert!(matches!(
            events[4],
            Event::ParamValue(ParamEvent { value: 0.2, .. })
        ));
        assert!(matches!(
            events[5],
            Event::ParamValue(ParamEvent { value: 0.9, .. })
        ));
    }

    #[test]
    fn an_absent_note_id_sorts_before_a_present_one() {
        let mut events = [
            Event::ParamValue(ParamEvent {
                time: 0,
                track: 1,
                param_id: 1,
                target: Target::note(3, 60),
                value: 1.0,
            }),
            param(1, 0.0),
        ];
        assert!(sort_slice(&mut events));
        assert_eq!(events[0].note_id(), None);
        assert_eq!(events[1].note_id(), Some(3));
    }

    #[test]
    fn a_short_workspace_leaves_the_events_unmoved() {
        let mut events = [
            Event::note_on(1, 0, 1, 60, 1.0),
            Event::note_on(0, 0, 2, 60, 1.0),
        ];
        let before = events;
        assert!(!sort_events(&mut events, &mut [], &mut []));
        assert_eq!(events, before);
    }

    #[test]
    fn reverse_tracks_at_one_time_keep_track_order_and_ties() {
        let mut events: Vec<_> = (0..32u32)
            .rev()
            .flat_map(|track| {
                [
                    Event::note_on(0, track, track, 60, 0.2),
                    Event::note_off(0, track, track, 60),
                    Event::note_on(0, track, track, 60, 0.8),
                ]
            })
            .collect();
        assert!(sort_slice(&mut events));
        let (offs, ons) = events.split_at(32);
        for (index, event) in offs.iter().enumerate() {
            let track = index as u32;
            assert!(event.is_note_off(), "track {track}");
            assert_eq!(event.track_key(), Some(track));
            assert_eq!(event.time(), 0);
        }
        for (index, pair) in ons.chunks(2).enumerate() {
            let track = index as u32;
            assert!(pair[0].is_note_on());
            assert_eq!(pair[0].velocity(), 0.2);
            assert!(pair[1].is_note_on());
            assert_eq!(pair[1].velocity(), 0.8);
            assert!(pair.iter().all(|event| event.track_key() == Some(track)));
            assert!(pair.iter().all(|event| event.time() == 0));
        }
    }
}
