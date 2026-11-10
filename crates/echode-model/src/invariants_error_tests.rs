use super::*;
use crate::ArrangeError;
use std::error::Error;
use std::sync::Arc;

#[test]
fn assembly_and_validation_keep_the_same_rule_and_numeric_bounds() {
    let (content, placement) = clip(2, ClipKind::Auto, 0, 2);
    let tracks = vec![track(1, None, vec![placement])];
    let checked = project(vec![content.clone()], tracks.clone())
        .check_invariants()
        .unwrap_err();
    let bar = Meter::four_four().bar_ticks().unwrap().get();
    assert_eq!(
        checked,
        InvariantError::AutoBounds {
            clip: Id::from_raw(2),
            start: 0,
            end: 2 * bar,
            low: 0,
            high: 0,
        }
    );
    let error = assemble(vec![content], tracks).unwrap_err();
    assert_eq!(error, ModelError::Invariant(checked.clone()));
    assert_eq!(error.clone(), error);
    assert_eq!(
        error.source().unwrap().downcast_ref::<InvariantError>(),
        Some(&checked)
    );
    assert_eq!(error.to_string(), checked.to_string());
    assert!(checked.source().is_none());
}

#[test]
fn order_causes_keep_ids_even_when_track_names_are_identical() {
    let mut first = track(1, None, Vec::new());
    let mut second = track(2, None, Vec::new());
    first.name = "same name, even with an order key".into();
    second.name = first.name.clone();
    second.order = first.order.clone();
    let expected = ArrangeError::DuplicateKey {
        first: first.id,
        first_name: first.name.clone(),
        second: second.id,
        second_name: second.name.clone(),
    };
    let checked = project(Vec::new(), vec![first.clone(), second.clone()])
        .check_invariants()
        .unwrap_err();
    assert_eq!(checked, InvariantError::Order(Box::new(expected.clone())));
    assert_eq!(
        checked.source().unwrap().downcast_ref::<ArrangeError>(),
        Some(&expected)
    );
    let error = assemble(Vec::new(), vec![first, second]).unwrap_err();
    assert_eq!(error, ModelError::Order(Box::new(expected.clone())));
    assert_eq!(
        error.source().unwrap().downcast_ref::<ArrangeError>(),
        Some(&expected)
    );
    assert_eq!(
        checked.to_string(),
        "tracks same name, even with an order key and same name, even with an order key share an order key"
    );
}

#[test]
fn derived_order_keeps_invalid_unreachable_and_repeated_track_causes() {
    let mut invalid = track(1, None, Vec::new());
    invalid.order = "not-a-key".into();
    let cases = [
        (
            vec![invalid],
            ArrangeError::InvalidKey {
                track: Id::from_raw(1),
                name: "track 1".into(),
            },
            "track track 1 has an order key that is not a fractional index",
        ),
        (
            vec![track(1, Some(2), Vec::new())],
            ArrangeError::MissingTrack {
                track: Id::from_raw(1),
                name: "track 1".into(),
            },
            "track track 1 is not in the arrange order",
        ),
        (
            vec![track(1, None, Vec::new()), {
                let mut repeated = track(1, None, Vec::new());
                repeated.order = crate::model::sibling_order(2);
                repeated
            }],
            ArrangeError::Cycle {
                track: Id::from_raw(1),
                name: "track 1".into(),
            },
            "track track 1 is its own ancestor",
        ),
    ];
    for (tracks, expected, message) in cases {
        let tracks: Vec<_> = tracks.into_iter().map(Arc::new).collect();
        let error = crate::model::arrange_tracks(&tracks).unwrap_err();
        assert_eq!(error, ModelError::Order(Box::new(expected.clone())));
        assert_eq!(error.source().unwrap().to_string(), message);
        assert_eq!(error.source().unwrap().downcast_ref(), Some(&expected));
    }
}

#[test]
fn hierarchy_and_duplicate_id_failures_keep_the_affected_ids() {
    let orphan = project(Vec::new(), vec![track(1, Some(9), Vec::new())]);
    assert_eq!(
        orphan.check_invariants(),
        Err(InvariantError::MissingParent {
            track: Id::from_raw(1),
            parent: Id::from_raw(9),
        })
    );
    let cycle = project(
        Vec::new(),
        vec![track(1, Some(2), Vec::new()), track(2, Some(1), Vec::new())],
    );
    assert_eq!(
        cycle.check_invariants(),
        Err(InvariantError::TrackCycle {
            track: Id::from_raw(1),
        })
    );
    let (content, placement) = clip(2, ClipKind::Fixed, 0, 1);
    let duplicate = project(
        vec![content],
        vec![track(1, None, vec![placement]), track(2, None, Vec::new())],
    );
    assert_eq!(
        duplicate.check_invariants(),
        Err(InvariantError::DuplicateId {
            id: Id::from_raw(2),
        })
    );
}
