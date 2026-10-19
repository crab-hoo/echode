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
