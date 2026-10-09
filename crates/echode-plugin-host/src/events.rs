//! Turns a block's [`Event`]s into the CLAP input buffer.
//!
//! `None` in a destination becomes CLAP's -1. The buffer belongs to the caller
//! so a reused buffer does not allocate on later blocks.

use clack_host::events::event_types::TransportFlags;
use clack_host::events::event_types::{
    NoteChokeEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent,
    ParamModEvent, ParamValueEvent, TransportEvent,
};
use clack_host::events::io::EventBuffer;
use clack_host::events::{EventFlags, EventHeader, Pckn};
use clack_host::utils::{BeatTime, ClapId, SecondsTime};
use echode_realtime::{Event, ExpressionKind, Target, Transport};

/// Appends `events` to `buffer` in order. The caller clears `buffer` first.
pub fn write_input(events: &[Event], buffer: &mut EventBuffer) {
    for event in events {
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
            Event::ParamValue(note) => {
                buffer.push(&ParamValueEvent::new(
                    note.time,
                    ClapId::new(note.param_id),
                    pckn(note.target),
                    note.value,
                ));
            }
            Event::ParamMod(note) => {
                buffer.push(&ParamModEvent::new(
                    note.time,
                    ClapId::new(note.param_id),
                    pckn(note.target),
                    note.value,
                ));
            }
            Event::Transport(note) => buffer.push(&transport(note)),
        }
    }
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
mod tests {
    use super::write_input;
    use clack_host::events::Event as ClapEvent;
    use clack_host::events::io::{EventBuffer, InputEvents};
    use clack_host::events::spaces::CoreEventSpace;
    use echode_realtime::{Event, ParamEvent, Target};

    #[test]
    fn the_host_receives_the_same_note_and_an_unspecified_note_id() {
        let events = [
            Event::note_on(3, 1, 7, 60, 1.0),
            Event::ParamValue(ParamEvent {
                time: 3,
                track: 1,
                param_id: 2,
                target: Target::any(),
                value: 0.25,
            }),
        ];
        let mut buffer = EventBuffer::new();
        write_input(&events, &mut buffer);
        let input = InputEvents::from_buffer(&buffer);
        assert_eq!(input.len(), 2);
        let Some(CoreEventSpace::NoteOn(note)) = input[0].as_core_event() else {
            panic!("note on");
        };
        assert_eq!(ClapEvent::time(note), 3);
        assert_eq!(note.velocity(), 1.0);
        assert_eq!(note.key(), clack_host::events::Match::Specific(60));
        assert_eq!(note.note_id(), clack_host::events::Match::Specific(7));
        let Some(CoreEventSpace::ParamValue(param)) = input[1].as_core_event() else {
            panic!("param");
        };
        assert_eq!(param.note_id(), clack_host::events::Match::All);
        assert_eq!(param.param_id(), clack_host::utils::ClapId::new(2));
    }
}
