//! Plain records for events that cross a process boundary.
//!
//! Shared memory is untrusted: a plugin can write any bit pattern. `Event` is a
//! Rust enum, so a typed read of those bytes is undefined. Callers copy one
//! fixed-size record and decode it field by field. A bad tag, a bad bool, or a
//! short buffer is `None`.

use echode_realtime::{
    ChokeEvent, Event, ExpressionEvent, ExpressionKind, NoteEvent, ParamEvent, Target, Transport,
};

pub const RECORD_BYTES: usize = 128;

const OFF_TAG: usize = 0;
const OFF_TIME: usize = 4;
const OFF_TRACK: usize = 8;
const OFF_PORT: usize = 12;
const OFF_CHANNEL: usize = 16;
const OFF_KEY: usize = 20;
const OFF_NOTE: usize = 24;
const OFF_FLAGS: usize = 28;
const OFF_EXTRA: usize = 32;
const OFF_SIG: usize = 36;
const OFF_F64: usize = 48;

const TAG_TRANSPORT: u32 = 0;
const TAG_NOTE_ON: u32 = 1;
const TAG_NOTE_OFF: u32 = 2;
const TAG_CHOKE: u32 = 3;
const TAG_EXPRESSION: u32 = 4;
const TAG_PARAM: u32 = 5;
const TAG_MOD: u32 = 6;

const NONE: u32 = u32::MAX;

/// Encode `event` into a zeroed record. `Some(u32::MAX)` cannot be a note id;
/// that bit pattern is the empty target.
pub fn encode_event(event: &Event) -> Option<[u8; RECORD_BYTES]> {
    let mut record = [0u8; RECORD_BYTES];
    match *event {
        Event::Transport(transport) => {
            write_u32(&mut record, OFF_TAG, TAG_TRANSPORT);
            write_u32(&mut record, OFF_TIME, transport.time);
            write_flag(&mut record, 0, transport.playing)?;
            write_flag(&mut record, 1, transport.recording)?;
            write_flag(&mut record, 2, transport.loop_active)?;
            write_u32(&mut record, OFF_EXTRA, transport.bar_number as u32);
            let signature = u32::from(transport.signature_numerator)
                | (u32::from(transport.signature_denominator) << 16);
            write_u32(&mut record, OFF_SIG, signature);
            write_f64(&mut record, 0, transport.tempo_bpm);
            write_f64(&mut record, 1, transport.tempo_inc);
            write_f64(&mut record, 2, transport.song_pos_beats);
            write_f64(&mut record, 3, transport.song_pos_seconds);
            write_f64(&mut record, 4, transport.loop_start_beats);
            write_f64(&mut record, 5, transport.loop_end_beats);
            write_f64(&mut record, 6, transport.loop_start_seconds);
            write_f64(&mut record, 7, transport.loop_end_seconds);
            write_f64(&mut record, 8, transport.bar_start);
        }
        Event::NoteOn(note) => write_note(&mut record, TAG_NOTE_ON, &note)?,
        Event::NoteOff(note) => write_note(&mut record, TAG_NOTE_OFF, &note)?,
        Event::NoteChoke(choke) => {
            write_u32(&mut record, OFF_TAG, TAG_CHOKE);
            write_u32(&mut record, OFF_TIME, choke.time);
            write_u32(&mut record, OFF_TRACK, choke.track);
            write_target(&mut record, choke.target)?;
        }
        Event::Expression(expression) => {
            write_u32(&mut record, OFF_TAG, TAG_EXPRESSION);
            write_u32(&mut record, OFF_TIME, expression.time);
            write_u32(&mut record, OFF_TRACK, expression.track);
            write_target(&mut record, expression.target)?;
            write_u32(&mut record, OFF_FLAGS, kind_tag(expression.kind));
            write_f64(&mut record, 0, expression.value);
        }
        Event::ParamValue(param) => write_param(&mut record, TAG_PARAM, &param)?,
        Event::ParamMod(param) => write_param(&mut record, TAG_MOD, &param)?,
    }
    Some(record)
}

/// Decode one record. Invalid tags, bools, targets, and short slices are `None`.
pub fn decode_event(src: &[u8]) -> Option<Event> {
    if src.len() < RECORD_BYTES {
        return None;
    }
    let tag = read_u32(src, OFF_TAG)?;
    let time = read_u32(src, OFF_TIME)?;
    let track = read_u32(src, OFF_TRACK)?;
    Some(match tag {
        TAG_TRANSPORT => Event::Transport(Transport {
            time,
            playing: read_flag(src, OFF_FLAGS)?,
            recording: read_flag(src, OFF_FLAGS + 1)?,
            loop_active: read_flag(src, OFF_FLAGS + 2)?,
            tempo_bpm: read_f64(src, 0)?,
            tempo_inc: read_f64(src, 1)?,
            song_pos_beats: read_f64(src, 2)?,
            song_pos_seconds: read_f64(src, 3)?,
            loop_start_beats: read_f64(src, 4)?,
            loop_end_beats: read_f64(src, 5)?,
            loop_start_seconds: read_f64(src, 6)?,
            loop_end_seconds: read_f64(src, 7)?,
            bar_start: read_f64(src, 8)?,
            bar_number: read_u32(src, OFF_EXTRA)? as i32,
            signature_numerator: read_u32(src, OFF_SIG)? as u16,
            signature_denominator: (read_u32(src, OFF_SIG)? >> 16) as u16,
        }),
        TAG_NOTE_ON => Event::NoteOn(read_note(src, time, track)?),
        TAG_NOTE_OFF => Event::NoteOff(read_note(src, time, track)?),
        TAG_CHOKE => Event::NoteChoke(ChokeEvent {
            time,
            track,
            target: read_target(src)?,
        }),
        TAG_EXPRESSION => Event::Expression(ExpressionEvent {
            time,
            track,
            target: read_target(src)?,
            kind: read_kind(read_u32(src, OFF_FLAGS)?)?,
            value: read_f64(src, 0)?,
        }),
        TAG_PARAM => Event::ParamValue(read_param(src, time, track)?),
        TAG_MOD => Event::ParamMod(read_param(src, time, track)?),
        _ => return None,
    })
}

fn write_note(record: &mut [u8; RECORD_BYTES], tag: u32, note: &NoteEvent) -> Option<()> {
    write_u32(record, OFF_TAG, tag);
    write_u32(record, OFF_TIME, note.time);
    write_u32(record, OFF_TRACK, note.track);
    write_target(record, note.target)?;
    write_f64(record, 0, note.velocity);
    Some(())
}

fn write_param(record: &mut [u8; RECORD_BYTES], tag: u32, param: &ParamEvent) -> Option<()> {
    write_u32(record, OFF_TAG, tag);
    write_u32(record, OFF_TIME, param.time);
    write_u32(record, OFF_TRACK, param.track);
    write_target(record, param.target)?;
    write_u32(record, OFF_EXTRA, param.param_id);
    write_f64(record, 0, param.value);
    Some(())
}

fn read_note(src: &[u8], time: u32, track: u32) -> Option<NoteEvent> {
    if read_u32(src, OFF_FLAGS)? != 0 {
        return None;
    }
    Some(NoteEvent {
        time,
        track,
        target: read_target(src)?,
        velocity: read_f64(src, 0)?,
    })
}

fn read_param(src: &[u8], time: u32, track: u32) -> Option<ParamEvent> {
    if read_u32(src, OFF_FLAGS)? != 0 {
        return None;
    }
    Some(ParamEvent {
        time,
        track,
        param_id: read_u32(src, OFF_EXTRA)?,
        target: read_target(src)?,
        value: read_f64(src, 0)?,
    })
}

fn write_target(record: &mut [u8; RECORD_BYTES], target: Target) -> Option<()> {
    write_u32(record, OFF_PORT, opt_u16(target.port)?);
    write_u32(record, OFF_CHANNEL, opt_u16(target.channel)?);
    write_u32(record, OFF_KEY, opt_u8(target.key)?);
    write_u32(record, OFF_NOTE, opt_u32(target.note_id)?);
    Some(())
}

fn read_target(src: &[u8]) -> Option<Target> {
    Some(Target {
        port: read_opt_u16(read_u32(src, OFF_PORT)?)?,
        channel: read_opt_u16(read_u32(src, OFF_CHANNEL)?)?,
        key: read_opt_u8(read_u32(src, OFF_KEY)?)?,
        note_id: read_opt_u32(read_u32(src, OFF_NOTE)?)?,
    })
}

fn opt_u32(value: Option<u32>) -> Option<u32> {
    match value {
        None => Some(NONE),
        Some(NONE) => None,
        Some(value) => Some(value),
    }
}

fn opt_u16(value: Option<u16>) -> Option<u32> {
    Some(match value {
        None => NONE,
        Some(value) => u32::from(value),
    })
}

fn opt_u8(value: Option<u8>) -> Option<u32> {
    Some(match value {
        None => NONE,
        Some(value) => u32::from(value),
    })
}

fn read_opt_u32(raw: u32) -> Option<Option<u32>> {
    if raw == NONE {
        Some(None)
    } else {
        Some(Some(raw))
    }
}

fn read_opt_u16(raw: u32) -> Option<Option<u16>> {
    if raw == NONE {
        Some(None)
    } else if raw <= u32::from(u16::MAX) {
        Some(Some(raw as u16))
    } else {
        None
    }
}

fn read_opt_u8(raw: u32) -> Option<Option<u8>> {
    if raw == NONE {
        Some(None)
    } else if raw <= u32::from(u8::MAX) {
        Some(Some(raw as u8))
    } else {
        None
    }
}

fn kind_tag(kind: ExpressionKind) -> u32 {
    match kind {
        ExpressionKind::Pitch => 0,
        ExpressionKind::Timbre => 1,
        ExpressionKind::Pressure => 2,
    }
}

fn read_kind(tag: u32) -> Option<ExpressionKind> {
    Some(match tag {
        0 => ExpressionKind::Pitch,
        1 => ExpressionKind::Timbre,
        2 => ExpressionKind::Pressure,
        _ => return None,
    })
}

fn write_flag(record: &mut [u8; RECORD_BYTES], index: usize, value: bool) -> Option<()> {
    *record.get_mut(OFF_FLAGS + index)? = u8::from(value);
    Some(())
}

fn read_flag(src: &[u8], at: usize) -> Option<bool> {
    match src.get(at).copied()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn write_u32(record: &mut [u8], at: usize, value: u32) {
    record[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(src: &[u8], at: usize) -> Option<u32> {
    let bytes = src.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn write_f64(record: &mut [u8], index: usize, value: f64) {
    let at = OFF_F64 + index * 8;
    record[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_f64(src: &[u8], index: usize) -> Option<f64> {
    let at = OFF_F64 + index * 8;
    let bytes = src.get(at..at + 8)?.try_into().ok()?;
    Some(f64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tag_on: bool) -> Event {
        let note = NoteEvent {
            time: 3,
            track: 4,
            target: Target::note(9, 60),
            velocity: 0.5,
        };
        if tag_on {
            Event::NoteOn(note)
        } else {
            Event::NoteOff(note)
        }
    }

    #[test]
    fn every_event_round_trips() {
        let events = [
            Event::Transport(Transport {
                time: 1,
                playing: true,
                recording: false,
                loop_active: true,
                tempo_bpm: 120.0,
                tempo_inc: 0.0,
                song_pos_beats: 1.5,
                song_pos_seconds: 0.75,
                loop_start_beats: 0.0,
                loop_end_beats: 4.0,
                loop_start_seconds: 0.0,
                loop_end_seconds: 2.0,
                bar_start: 0.0,
                bar_number: -2,
                signature_numerator: 4,
                signature_denominator: 4,
            }),
            note(true),
            note(false),
            Event::NoteChoke(ChokeEvent {
                time: 8,
                track: 1,
                target: Target::any(),
            }),
            Event::Expression(ExpressionEvent {
                time: 2,
                track: 1,
                target: Target::note(3, 61),
                kind: ExpressionKind::Pressure,
                value: 0.25,
            }),
            Event::ParamValue(ParamEvent {
                time: 0,
                track: 2,
                param_id: 7,
                target: Target::any(),
                value: 0.125,
            }),
            Event::ParamMod(ParamEvent {
                time: 5,
                track: 2,
                param_id: 8,
                target: Target::any(),
                value: -0.5,
            }),
        ];
        for event in events {
            let record = encode_event(&event).expect("encode");
            assert_eq!(decode_event(&record), Some(event));
        }
    }

    #[test]
    fn invalid_records_are_none() {
        assert!(decode_event(&[0; 16]).is_none());
        let mut record = encode_event(&note(true)).expect("encode");
        record[OFF_TAG] = 99;
        assert!(decode_event(&record).is_none());
        let mut transport = encode_event(&Event::Transport(Transport {
            time: 0,
            playing: true,
            recording: false,
            loop_active: false,
            tempo_bpm: 100.0,
            tempo_inc: 0.0,
            song_pos_beats: 0.0,
            song_pos_seconds: 0.0,
            loop_start_beats: 0.0,
            loop_end_beats: 0.0,
            loop_start_seconds: 0.0,
            loop_end_seconds: 0.0,
            bar_start: 0.0,
            bar_number: 0,
            signature_numerator: 4,
            signature_denominator: 4,
        }))
        .expect("encode");
        transport[OFF_FLAGS] = 2;
        assert!(decode_event(&transport).is_none());
        let mut expression = encode_event(&Event::Expression(ExpressionEvent {
            time: 0,
            track: 0,
            target: Target::any(),
            kind: ExpressionKind::Pitch,
            value: 0.0,
        }))
        .expect("encode");
        expression[OFF_FLAGS] = 9;
        assert!(decode_event(&expression).is_none());
        assert!(
            encode_event(&Event::NoteOn(NoteEvent {
                time: 0,
                track: 0,
                target: Target {
                    port: None,
                    channel: None,
                    key: None,
                    note_id: Some(u32::MAX),
                },
                velocity: 1.0,
            }))
            .is_none()
        );
    }
}
