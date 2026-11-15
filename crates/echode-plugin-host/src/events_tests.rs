use super::write_input;
use crate::note_ports::NoteDialect::{Clap, Midi, Midi2};
use clack_host::events::Event as ClapEvent;
use clack_host::events::io::{EventBuffer, InputEvents};
use clack_host::events::spaces::CoreEventSpace;
use echode_realtime::{
    ChokeEvent, Event, ExpressionEvent, ExpressionKind, NoteEvent, ParamEvent, Target, Transport,
};

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
    write_input(&events, &[], &mut buffer);
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

#[test]
fn the_invalid_clap_parameter_id_is_skipped() {
    let invalid = ParamEvent {
        time: 0,
        track: 1,
        param_id: u32::MAX,
        target: Target::any(),
        value: 0.5,
    };
    let mut buffer = EventBuffer::new();
    write_input(
        &[Event::ParamValue(invalid), Event::ParamMod(invalid)],
        &[],
        &mut buffer,
    );
    assert_eq!(InputEvents::from_buffer(&buffer).len(), 0);
}

fn aimed(port: Option<u16>, channel: Option<u16>, key: Option<u8>) -> Target {
    Target {
        port,
        channel,
        key,
        note_id: Some(4),
    }
}

fn on(time: u32, target: Target, velocity: f64) -> Event {
    Event::NoteOn(NoteEvent {
        time,
        track: 0,
        target,
        velocity,
    })
}

fn off(time: u32, target: Target, velocity: f64) -> Event {
    Event::NoteOff(NoteEvent {
        time,
        track: 0,
        target,
        velocity,
    })
}

fn expression(time: u32, target: Target, kind: ExpressionKind, value: f64) -> Event {
    Event::Expression(ExpressionEvent {
        time,
        track: 0,
        target,
        kind,
        value,
    })
}

/// `(time, port, bytes)` for MIDI, `(time, port, [0xFF, key, 0])` for native
/// notes, and `[0xFE, 0, 0]` / `[0xFD, 0, 0]` for expressions / other events.
fn delivered(buffer: &EventBuffer) -> Vec<(u32, u16, [u8; 3])> {
    use clack_host::events::Pckn;
    let native = |time, pckn: Pckn, tag| {
        let port = pckn.port_index.into_specific().unwrap_or(u16::MAX);
        let key = pckn.key.into_specific().unwrap_or(0xFF);
        (time, port, [tag, key as u8, 0])
    };
    InputEvents::from_buffer(buffer)
        .iter()
        .map(|event| {
            let time = event.header().time();
            match event.as_core_event() {
                Some(CoreEventSpace::Midi(midi)) => (time, midi.port_index(), midi.data()),
                Some(CoreEventSpace::NoteOn(note)) => native(time, note.pckn(), 0xFF),
                Some(CoreEventSpace::NoteOff(note)) => native(time, note.pckn(), 0xFF),
                Some(CoreEventSpace::NoteChoke(note)) => native(time, note.pckn(), 0xFF),
                Some(CoreEventSpace::NoteExpression(note)) => {
                    let (time, port, _) = native(time, note.pckn(), 0xFE);
                    (time, port, [0xFE, 0, 0])
                }
                _ => (time, u16::MAX, [0xFD, 0, 0]),
            }
        })
        .collect()
}

#[test]
fn midi_only_ports_receive_midi_1_channel_messages_in_order() {
    let midi = |channel, key| aimed(Some(1), Some(channel), Some(key));
    let events = [
        Event::Transport(Transport {
            time: 0,
            playing: true,
            recording: false,
            loop_active: false,
            tempo_bpm: 120.0,
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
        }),
        on(2, aimed(Some(0), Some(0), Some(60)), 1.0),
        on(3, midi(2, 61), 0.0),
        on(3, midi(15, 127), 1.0),
        expression(4, midi(2, 61), ExpressionKind::Pressure, 0.5),
        expression(4, midi(2, 61), ExpressionKind::Pitch, 2.0),
        expression(4, midi(2, 61), ExpressionKind::Timbre, 0.5),
        off(5, midi(2, 61), 0.25),
        Event::NoteChoke(ChokeEvent {
            time: 6,
            track: 0,
            target: midi(15, 127),
        }),
        // One MIDI message cannot name several channels or keys, or a 17th channel.
        on(6, aimed(Some(1), None, Some(62)), 1.0),
        on(6, aimed(Some(1), Some(0), None), 1.0),
        on(6, aimed(Some(1), Some(16), Some(62)), 1.0),
        // A wildcard port stays native for the CLAP ports it also matches.
        off(7, aimed(None, Some(0), Some(60)), 0.0),
        // Ports past the declared list are native.
        on(7, aimed(Some(2), Some(0), Some(64)), 1.0),
        Event::ParamValue(ParamEvent {
            time: 7,
            track: 0,
            param_id: 3,
            target: midi(2, 61),
            value: 0.5,
        }),
    ];
    let mut buffer = EventBuffer::with_capacity(events.len());
    {
        let _guard = crate::realtime::ForbidAlloc::enter();
        write_input(&events, &[Clap, Midi], &mut buffer);
    }
    assert_eq!(
        delivered(&buffer),
        [
            (0, u16::MAX, [0xFD, 0, 0]),
            (2, 0, [0xFF, 60, 0]),
            (3, 1, [0x92, 61, 1]),
            (3, 1, [0x9F, 127, 127]),
            (4, 1, [0xA2, 61, 64]),
            (5, 1, [0x82, 61, 32]),
            (6, 1, [0x8F, 127, 0]),
            (7, u16::MAX, [0xFF, 60, 0]),
            (7, 2, [0xFF, 64, 0]),
            (7, u16::MAX, [0xFD, 0, 0]),
        ]
    );
}

#[test]
fn every_stored_midi_velocity_survives_the_round_trip() {
    let port = [Midi];
    for midi in 0..=127u8 {
        let velocity = f64::from(echode_model::midi7_to_velocity(midi)) / 65535.0;
        let target = aimed(Some(0), Some(0), Some(60));
        let mut buffer = EventBuffer::new();
        write_input(
            &[on(0, target, velocity), off(1, target, velocity)],
            &port,
            &mut buffer,
        );
        assert_eq!(
            delivered(&buffer),
            [(0, 0, [0x90, 60, midi.max(1)]), (1, 0, [0x80, 60, midi])]
        );
    }
}

#[test]
fn out_of_range_and_nan_velocities_stay_valid_midi() {
    let target = aimed(Some(0), Some(0), Some(60));
    let mut buffer = EventBuffer::new();
    write_input(
        &[
            on(0, target, -1.0),
            on(0, target, 2.0),
            on(0, target, f64::NAN),
            off(1, target, f64::INFINITY),
            off(1, target, f64::NAN),
        ],
        &[Midi],
        &mut buffer,
    );
    assert_eq!(
        delivered(&buffer),
        [
            (0, 0, [0x90, 60, 1]),
            (0, 0, [0x90, 60, 127]),
            (0, 0, [0x90, 60, 1]),
            (1, 0, [0x80, 60, 127]),
            (1, 0, [0x80, 60, 0]),
        ]
    );
}

fn ump(buffer: &EventBuffer) -> Vec<(u32, u16, [u32; 4])> {
    InputEvents::from_buffer(buffer)
        .iter()
        .filter_map(|event| {
            let CoreEventSpace::Midi2(midi) = event.as_core_event()? else {
                return None;
            };
            Some((event.header().time(), midi.port_index(), midi.data()))
        })
        .collect()
}

#[test]
fn midi2_notes_use_group_zero_and_full_resolution_without_allocating() {
    let target = aimed(Some(2), Some(15), Some(127));
    let events = [
        on(0, aimed(Some(0), Some(0), Some(60)), 1.0),
        on(1, aimed(Some(1), Some(0), Some(61)), 0.5),
        on(2, target, 0.0),
        on(2, target, 1.0),
        off(3, target, 32768.0 / 65535.0),
        expression(4, target, ExpressionKind::Pressure, 0.5),
        expression(4, target, ExpressionKind::Pitch, 2.0),
        expression(4, target, ExpressionKind::Timbre, 0.5),
        Event::NoteChoke(ChokeEvent {
            time: 5,
            track: 0,
            target,
        }),
        on(6, aimed(Some(2), None, Some(60)), 1.0),
        on(6, aimed(Some(2), Some(16), Some(60)), 1.0),
        on(6, aimed(Some(2), Some(0), None), 1.0),
        on(6, aimed(Some(2), Some(0), Some(128)), 1.0),
        // Wildcard and undeclared ports keep the native path.
        off(7, aimed(None, Some(0), Some(60)), 0.0),
        on(7, aimed(Some(3), Some(0), Some(60)), 1.0),
        Event::ParamValue(ParamEvent {
            time: 7,
            track: 0,
            param_id: 3,
            target,
            value: 0.5,
        }),
    ];
    let mut buffer = EventBuffer::with_capacity(events.len());
    {
        let _guard = crate::realtime::ForbidAlloc::enter();
        write_input(&events, &[Clap, Midi, Midi2], &mut buffer);
    }
    assert_eq!(InputEvents::from_buffer(&buffer).len(), 10);
    assert_eq!(
        ump(&buffer),
        [
            (2, 2, [0x409F_7F00, 0, 0, 0]),
            (2, 2, [0x409F_7F00, 0xFFFF_0000, 0, 0]),
            (3, 2, [0x408F_7F00, 0x8000_0000, 0, 0]),
            (4, 2, [0x40AF_7F00, 0x8000_0000, 0, 0]),
            (5, 2, [0x408F_7F00, 0, 0, 0]),
        ]
    );
    let delivered = delivered(&buffer);
    assert_eq!(delivered[0], (0, 0, [0xFF, 60, 0]));
    assert_eq!(delivered[1], (1, 1, [0x90, 61, 64]));
    assert_eq!(
        &delivered[7..],
        &[
            (7, u16::MAX, [0xFF, 60, 0]),
            (7, 3, [0xFF, 60, 0]),
            (7, u16::MAX, [0xFD, 0, 0]),
        ]
    );
}

#[test]
fn every_model_velocity_survives_midi2_and_zero_is_a_note_on() {
    let mut buffer = EventBuffer::with_capacity(2);
    let target = aimed(Some(0), Some(0), Some(60));
    for velocity in 0..=u16::MAX {
        buffer.clear();
        let unit = f64::from(velocity) / 65535.0;
        write_input(
            &[on(0, target, unit), off(1, target, unit)],
            &[Midi2],
            &mut buffer,
        );
        assert_eq!(
            ump(&buffer),
            [
                (0, 0, [0x4090_3C00, u32::from(velocity) << 16, 0, 0]),
                (1, 0, [0x4080_3C00, u32::from(velocity) << 16, 0, 0]),
            ]
        );
    }
}

#[test]
fn midi2_velocities_and_pressure_clamp_nonfinite_and_out_of_range_values() {
    let mut buffer = EventBuffer::with_capacity(3);
    let target = aimed(Some(0), Some(0), Some(60));
    for (value, velocity, pressure) in [
        (-1.0, 0, 0),
        (f64::NEG_INFINITY, 0, 0),
        (f64::NAN, 0, 0),
        (2.0, u16::MAX, u32::MAX),
        (f64::INFINITY, u16::MAX, u32::MAX),
        (1.0, u16::MAX, u32::MAX),
    ] {
        buffer.clear();
        write_input(
            &[
                on(0, target, value),
                off(1, target, value),
                expression(2, target, ExpressionKind::Pressure, value),
            ],
            &[Midi2],
            &mut buffer,
        );
        assert_eq!(
            ump(&buffer),
            [
                (0, 0, [0x4090_3C00, u32::from(velocity) << 16, 0, 0]),
                (1, 0, [0x4080_3C00, u32::from(velocity) << 16, 0, 0]),
                (2, 0, [0x40A0_3C00, pressure, 0, 0]),
            ]
        );
    }
}
