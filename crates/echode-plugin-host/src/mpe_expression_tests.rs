use super::{delivered, expression, mpe_note, off, on};
use crate::events::{MAX_INPUT_EXPANSION, write_input};
use crate::mpe::MpeZone;
use crate::note_ports::NoteDialect::Mpe;
use clack_host::events::io::EventBuffer;
use echode_realtime::ExpressionKind::{Pitch, Pressure, Timbre};

#[test]
fn sharing_neutralizes_expression_and_blocks_both_owner_and_guest_expression() {
    let target = |id| mpe_note(Some(0), None, Some(60), Some(id));
    // Sixteen held notes exercise the protocol's fifteen-member boundary only.
    let mut events: Vec<_> = (0..15).map(|id| on(0, target(id), 1.0)).collect();
    events.extend([
        expression(1, target(0), Pitch, 24.0),
        on(2, target(15), 1.0),
        expression(3, target(0), Pitch, -24.0),
        expression(3, target(15), Pressure, 1.0),
        expression(4, target(1), Timbre, 0.0),
        off(5, target(15), 0.0),
        expression(6, target(0), Pressure, 0.5),
    ]);
    let mut zones = [MpeZone::new(0)];
    let mut buffer = EventBuffer::with_capacity(events.len() * MAX_INPUT_EXPANSION);
    {
        let _guard = crate::realtime::ForbidAlloc::enter();
        write_input(&events, &[Mpe], &mut zones, &mut buffer);
    }
    let seen = delivered(&buffer);
    assert_eq!(
        &seen[60..],
        &[
            (1, 0, [0xE1, 0, 96]),
            (2, 0, [0xE1, 0, 64]),
            (2, 0, [0xD1, 0, 0]),
            (2, 0, [0xB1, 74, 64]),
            (2, 0, [0x91, 60, 127]),
            (4, 0, [0xB2, 74, 0]),
            (5, 0, [0x81, 60, 0]),
            (6, 0, [0xD1, 64, 0]),
        ],
        "MPE shared channel leaked expression"
    );
}

#[test]
fn expression_bounds_remain_midi_and_nonfinite_values_are_not_sent() {
    let target = mpe_note(Some(0), None, None, Some(1));
    let mut events = vec![on(0, mpe_note(Some(0), None, Some(60), Some(1)), 1.0)];
    for (kind, value) in [
        (Pitch, -100.0),
        (Pitch, -48.0),
        (Pitch, 0.0),
        (Pitch, 48.0),
        (Pitch, 100.0),
        (Pressure, -1.0),
        (Pressure, 2.0),
        (Timbre, -1.0),
        (Timbre, 2.0),
    ] {
        events.push(expression(1, target, kind, value));
    }
    for kind in [Pitch, Pressure, Timbre] {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            events.push(expression(2, target, kind, value));
        }
    }
    events.push(expression(
        3,
        mpe_note(Some(0), None, None, None),
        Pressure,
        1.0,
    ));
    let mut zones = [MpeZone::new(0)];
    let mut buffer = EventBuffer::with_capacity(events.len() * MAX_INPUT_EXPANSION);
    {
        let _guard = crate::realtime::ForbidAlloc::enter();
        write_input(&events, &[Mpe], &mut zones, &mut buffer);
    }
    assert_eq!(
        &delivered(&buffer)[4..],
        &[
            (1, 0, [0xE1, 0, 0]),
            (1, 0, [0xE1, 0, 0]),
            (1, 0, [0xE1, 0, 64]),
            (1, 0, [0xE1, 127, 127]),
            (1, 0, [0xE1, 127, 127]),
            (1, 0, [0xD1, 0, 0]),
            (1, 0, [0xD1, 127, 0]),
            (1, 0, [0xB1, 74, 0]),
            (1, 0, [0xB1, 74, 127]),
        ]
    );
}

#[test]
fn a_block_full_of_note_ons_uses_only_prepared_translation_storage() {
    // This block density is this regression's input, not a product limit.
    let events: Vec<_> = (0..128)
        .map(|id| {
            on(
                0,
                mpe_note(Some(0), None, Some((id % 128) as u8), Some(id)),
                1.0,
            )
        })
        .collect();
    let mut zones = [MpeZone::new(0)];
    let mut buffer = EventBuffer::with_capacity(events.len() * MAX_INPUT_EXPANSION);
    {
        let _guard = crate::realtime::ForbidAlloc::enter();
        write_input(&events, &[Mpe], &mut zones, &mut buffer);
    }
    assert_eq!(buffer.len() as usize, events.len() * 4);
}
