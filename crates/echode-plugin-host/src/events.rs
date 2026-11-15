//! Turns a block's [`Event`]s into the CLAP input buffer.
//!
//! `None` in a destination becomes CLAP's -1. The buffer belongs to the caller
//! so a reused buffer does not allocate on later blocks.
//!
//! MIDI 1.0 and MIDI 2.0 ports receive at most one message per event; pitch
//! and brightness are not translated there. MPE notes use member channels,
//! with neutral expression values before each note-on. Per-note expressions
//! reach only exclusively owned channels, never other sounding notes.

use crate::mpe::{MEMBER_PITCH_RANGE, MpeZone};
use crate::note_ports::NoteDialect;
use clack_host::events::event_types::TransportFlags;
use clack_host::events::event_types::{
    Midi2Event, MidiEvent, NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent,
    NoteOnEvent, ParamModEvent, ParamValueEvent, TransportEvent,
};
use clack_host::events::io::EventBuffer;
use clack_host::events::{EventFlags, EventHeader, Pckn};
use clack_host::utils::{BeatTime, ClapId, SecondsTime};
use echode_realtime::{Event, ExpressionKind, Target, Transport};

/// An MPE note-on expands to three initial values and the note itself.
pub(crate) const MAX_INPUT_EXPANSION: usize = 4;

/// Appends `events` to `buffer` in order. The caller clears `buffer` first.
///
/// `dialects[port]` selects the note wire format. Missing entries stay native.
/// `zones` holds, in port order, the member channels of every MPE port.
/// A note for an MPE port without a zone is not sent.
pub fn write_input(
    events: &[Event],
    dialects: &[NoteDialect],
    zones: &mut [MpeZone],
    buffer: &mut EventBuffer,
) {
    for event in events {
        if let Some((port, dialect)) = midi_port(event, dialects) {
            // Translation preserves input order, including same-time initial values.
            match dialect {
                NoteDialect::Midi => {
                    if let Some(data) = midi_message(event) {
                        buffer.push(&MidiEvent::new(event.time(), port, data));
                    }
                }
                NoteDialect::Midi2 => {
                    if let Some(data) = midi2_message(event) {
                        buffer.push(&Midi2Event::new(event.time(), port, data));
                    }
                }
                NoteDialect::Mpe => {
                    let zone = zones
                        .binary_search_by_key(&port, MpeZone::port)
                        .ok()
                        .and_then(|index| zones.get_mut(index));
                    if let Some(zone) = zone {
                        write_mpe(event, zone, buffer);
                    }
                }
                NoteDialect::Clap => {}
            }
            continue;
        }
        match event {
            Event::NoteOn(note) => {
                buffer.push(&NoteOnEvent::new(
                    note.time,
                    pckn(note.target),
                    note.velocity,
                ));
            }
            Event::NoteOff(note) => {
                buffer.push(&NoteOffEvent::new(
                    note.time,
                    pckn(note.target),
                    note.velocity,
                ));
            }
            Event::NoteChoke(note) => {
                buffer.push(&NoteChokeEvent::new(note.time, pckn(note.target)));
            }
            Event::Expression(note) => {
                let kind = match note.kind {
                    ExpressionKind::Pitch => NoteExpressionType::Tuning,
                    ExpressionKind::Timbre => NoteExpressionType::Brightness,
                    ExpressionKind::Pressure => NoteExpressionType::Pressure,
                };
                buffer.push(&NoteExpressionEvent::new(
                    note.time,
                    pckn(note.target),
                    kind,
                    note.value,
                ));
            }
            // CLAP's invalid id would panic in `ClapId::new` on the audio thread.
            Event::ParamValue(note) => {
                if let Some(id) = ClapId::from_raw(note.param_id) {
                    buffer.push(&ParamValueEvent::new(
                        note.time,
                        id,
                        pckn(note.target),
                        note.value,
                    ));
                }
            }
            Event::ParamMod(note) => {
                if let Some(id) = ClapId::from_raw(note.param_id) {
                    buffer.push(&ParamModEvent::new(
                        note.time,
                        id,
                        pckn(note.target),
                        note.value,
                    ));
                }
            }
            Event::Transport(note) => buffer.push(&transport(note)),
        }
    }
}

/// The port of a note event bound for a MIDI-only port. A wildcard port stays
/// native: one MIDI message cannot address several ports.
fn midi_port(event: &Event, dialects: &[NoteDialect]) -> Option<(u16, NoteDialect)> {
    let target = match event {
        Event::NoteOn(note) | Event::NoteOff(note) => note.target,
        Event::NoteChoke(note) => note.target,
        Event::Expression(note) => note.target,
        Event::Transport(_) | Event::ParamValue(_) | Event::ParamMod(_) => return None,
    };
    let port = target.port?;
    let dialect = dialects.get(usize::from(port)).copied()?;
    (dialect != NoteDialect::Clap).then_some((port, dialect))
}

/// The MIDI 1.0 channel message for one note event, if it has one.
fn midi_message(event: &Event) -> Option<[u8; 3]> {
    let (status, target, value) = match event {
        // Velocity 0 would be a note-off, so the quietest note-on is 1.
        Event::NoteOn(note) => (0x90, note.target, seven_bits(note.velocity).max(1)),
        Event::NoteOff(note) => (0x80, note.target, seven_bits(note.velocity)),
        // MIDI 1.0 has no choke. Ending the note is the nearest message.
        Event::NoteChoke(note) => (0x80, note.target, 0),
        Event::Expression(note) if note.kind == ExpressionKind::Pressure => {
            (0xA0, note.target, seven_bits(note.value))
        }
        _ => return None,
    };
    let channel = target
        .channel
        .and_then(|channel| u8::try_from(channel).ok());
    let channel = channel.filter(|channel| *channel < 16)?;
    let key = target.key.filter(|key| *key < 128)?;
    Some([status | channel, key, value])
}

/// A note on its zone's member channel; the event's own channel is not used.
fn write_mpe(event: &Event, zone: &mut MpeZone, buffer: &mut EventBuffer) {
    let send = |buffer: &mut EventBuffer, data| {
        buffer.push(&MidiEvent::new(event.time(), zone.port(), data));
    };
    if let Event::Expression(note) = event {
        let Some(member) = note
            .target
            .note_id
            .and_then(|id| zone.exclusive_channel(id))
        else {
            return;
        };
        if !note.value.is_finite() {
            return;
        }
        let data = match note.kind {
            ExpressionKind::Pitch => {
                // The zone explicitly configures every member's bend range.
                let normalized = (note.value / f64::from(MEMBER_PITCH_RANGE)).clamp(-1.0, 1.0);
                let scale = if normalized < 0.0 { 8192.0 } else { 8191.0 };
                let bend = (8192.0 + normalized * scale).round() as u16;
                [0xE0 | member, (bend & 0x7F) as u8, (bend >> 7) as u8]
            }
            ExpressionKind::Pressure => [0xD0 | member, seven_bits(note.value), 0],
            ExpressionKind::Timbre => [0xB0 | member, 74, seven_bits(note.value)],
        };
        send(buffer, data);
        return;
    }
    let (on, target, value) = match event {
        Event::NoteOn(note) => (true, note.target, seven_bits(note.velocity).max(1)),
        Event::NoteOff(note) => (false, note.target, seven_bits(note.velocity)),
        Event::NoteChoke(note) => (false, note.target, 0),
        _ => return,
    };
    // Check the address before the zone counts a note.
    let (Some(key), Some(note_id)) = (target.key.filter(|key| *key < 128), target.note_id) else {
        return;
    };
    if on {
        let member = zone.start(note_id, key);
        let port = zone.port();
        // Reused channels must not inherit the previous note's expression. When
        // sharing begins, neutralize the channel and suppress further expression.
        for data in [
            [0xE0 | member, 0, 64],
            [0xD0 | member, 0, 0],
            [0xB0 | member, 74, 64],
            [0x90 | member, key, value],
        ] {
            buffer.push(&MidiEvent::new(event.time(), port, data));
        }
    } else if let Some(member) = zone.end(note_id, key) {
        buffer.push(&MidiEvent::new(
            event.time(),
            zone.port(),
            [0x80 | member, key, value],
        ));
    }
}

/// MIDI 2.0 channel voice UMP, group zero, no note attributes (M2-104 §7.4).
fn midi2_message(event: &Event) -> Option<[u32; 4]> {
    let (status, target, data) = match event {
        // Unlike MIDI 1.0, MIDI 2.0 note-on velocity zero is a real note-on.
        Event::NoteOn(note) => (
            0x90,
            note.target,
            u32::from(sixteen_bits(note.velocity)) << 16,
        ),
        Event::NoteOff(note) => (
            0x80,
            note.target,
            u32::from(sixteen_bits(note.velocity)) << 16,
        ),
        // Neither MIDI dialect has the native CLAP choke operation.
        Event::NoteChoke(note) => (0x80, note.target, 0),
        Event::Expression(note) if note.kind == ExpressionKind::Pressure => (
            0xA0,
            note.target,
            (note.value.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32,
        ),
        _ => return None,
    };
    let channel = target.channel.filter(|channel| *channel < 16)?;
    let key = target.key.filter(|key| *key < 128)?;
    Some([
        0x4000_0000 | (status | u32::from(channel)) << 16 | u32::from(key) << 8,
        data,
        0,
        0,
    ])
}

fn sixteen_bits(value: f64) -> u16 {
    // The model stores this exact resolution; NaN's saturating cast yields 0.
    (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16
}

/// CLAP's 0..=1 to MIDI's 0..=127 at the nearest step. Values outside clamp.
fn seven_bits(value: f64) -> u8 {
    // NaN survives `clamp`; the saturating cast turns it into 0.
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

fn pckn(target: Target) -> Pckn {
    Pckn::from_raw(
        target
            .port
            .and_then(|port| i16::try_from(port).ok())
            .unwrap_or(-1),
        target
            .channel
            .and_then(|channel| i16::try_from(channel).ok())
            .unwrap_or(-1),
        target.key.map(i16::from).unwrap_or(-1),
        target
            .note_id
            .and_then(|note| i32::try_from(note).ok())
            .unwrap_or(-1),
    )
}

fn transport(event: &Transport) -> TransportEvent {
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_BEATS_TIMELINE
        | TransportFlags::HAS_SECONDS_TIMELINE
        | TransportFlags::HAS_TIME_SIGNATURE;
    if event.playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    if event.recording {
        flags |= TransportFlags::IS_RECORDING;
    }
    if event.loop_active {
        flags |= TransportFlags::IS_LOOP_ACTIVE;
    }
    TransportEvent {
        header: EventHeader::new_core(event.time, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from(event.song_pos_beats),
        song_pos_seconds: SecondsTime::from(event.song_pos_seconds),
        tempo: event.tempo_bpm,
        tempo_inc: event.tempo_inc,
        loop_start_beats: BeatTime::from(event.loop_start_beats),
        loop_end_beats: BeatTime::from(event.loop_end_beats),
        loop_start_seconds: SecondsTime::from(event.loop_start_seconds),
        loop_end_seconds: SecondsTime::from(event.loop_end_seconds),
        bar_start: BeatTime::from(event.bar_start),
        bar_number: event.bar_number,
        time_signature_numerator: event.signature_numerator,
        time_signature_denominator: event.signature_denominator,
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
