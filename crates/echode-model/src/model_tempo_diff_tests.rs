//! Tempo counts bound these changeset fixtures only, not product maps.

use super::measured;
use crate::{
    Change, ChangeSet, Field, ObjectKey, ObjectKind, Project, Property, Tempo, TempoMap,
    TempoPoint, Tick, Value,
};
use std::collections::BTreeMap;

fn tempo(index: usize) -> Tempo {
    Tempo::rational(300 + index as u32, 7).unwrap()
}

fn project(points: &[(i64, Tempo)]) -> Project {
    Project::try_assemble(
        Project::new().meter(),
        TempoMap::try_from_points(
            points
                .iter()
                .rev()
                .map(|&(at, tempo)| TempoPoint {
                    at: Tick::new(at),
                    tempo,
                })
                .collect(),
        )
        .unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn oracle(before: &Project, after: &Project) -> Vec<Change> {
    let points = |project: &Project| -> BTreeMap<_, _> {
        project
            .tempo()
            .points()
            .iter()
            .map(|point| (point.at, point.tempo))
            .collect()
    };
    let previous = points(before);
    let next = points(after);
    let mut changes = Vec::new();
    for (&at, &value) in &next {
        if !previous.contains_key(&at) {
            changes.push(Change::Create {
                kind: ObjectKind::TempoPoint,
                id: ObjectKey::Tempo(at),
                fields: vec![Field {
                    property: Property::Tempo,
                    value: Value::Tempo(value),
                }],
            });
        }
    }
    for (&at, &value) in &next {
        if let Some(&old) = previous.get(&at)
            && old != value
        {
            changes.push(Change::Update {
                id: ObjectKey::Tempo(at),
                property: Property::Tempo,
                before: Value::Tempo(old),
                after: Value::Tempo(value),
            });
        }
    }
    for (&at, &value) in &previous {
        if !next.contains_key(&at) {
            changes.push(Change::Delete {
                kind: ObjectKind::TempoPoint,
                id: ObjectKey::Tempo(at),
                fields: vec![Field {
                    property: Property::Tempo,
                    value: Value::Tempo(value),
                }],
            });
        }
    }
    changes
}

fn diff(before: &Project, after: &Project, shared: bool) -> ChangeSet {
    let (changes, work) = measured(|| before.diff(after));
    let limit = if shared {
        0
    } else {
        before.tempo().points().len() + after.tempo().points().len()
    };
    assert!(
        work.diff_tempo_visits <= limit,
        "tempo diff visited {} points (limit {limit}, shared {shared})",
        work.diff_tempo_visits
    );
    assert_eq!(work.diff_note_visits, 0, "tempo diff walked shared notes");
    changes
}

fn check(before: &Project, after: &Project) {
    let original = before.clone();
    let changes = diff(before, after, false);
    assert_eq!(changes.changes(), oracle(before, after));
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    assert_eq!(before.apply_changeset(&changes).unwrap(), *after);
    assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), *before);
    assert_eq!(*before, original);
}

#[test]
fn shared_tempo_maps_skip_all_point_visits() {
    for count in [1, 64, 256] {
        let points: Vec<_> = (0..count)
            .map(|index| (index as i64 - count as i64 / 2, tempo(index)))
            .collect();
        let before = points
            .iter()
            .fold(super::fixture(4), |project, &(at, value)| {
                project.with_tempo(Tick::new(at), value)
            });
        assert!(diff(&before, &before.clone(), true).is_empty());
        let (after, _) = before.add_track("unrelated edit");
        let changes = diff(&before, &after, true);
        assert_eq!(before.apply_changeset(&changes).unwrap(), after);
        assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), before);
        let parameter = after.tracks()[0].devices[0].parameters[0].id;
        let mixed = after.set_parameter(parameter, 0.5).unwrap();
        let changes = diff(&after, &mixed, true);
        assert_eq!(changes.changes().len(), 1);
        assert_eq!(after.apply_changeset(&changes).unwrap(), mixed);
        assert_eq!(mixed.apply_changeset(&changes.invert()).unwrap(), after);
    }
}

#[test]
fn tempo_diffs_visit_each_point_at_most_once() {
    for count in [64, 256] {
        let points: Vec<_> = (0..count)
            .map(|index| (index as i64 * 4 - count as i64 * 2, tempo(index)))
            .collect();
        let before = project(&points);
        // Equal values assembled separately must not depend on Arc identity.
        check(&before, &project(&points));
        for index in [0, count / 2, count - 1] {
            let after = before.with_tempo(Tick::new(points[index].0), tempo(count));
            check(&before, &after);
        }
        let next: Vec<_> = points
            .iter()
            .enumerate()
            .filter_map(|(index, &(at, value))| match index % 4 {
                0 => None,
                1 => Some((at, tempo(count + index))),
                2 => Some((at, value)),
                _ => Some((at + 1, value)),
            })
            .collect();
        let after = project(&next);
        check(&before, &after);
        check(&after, &before);
        // Exhaust either input first, with or without any matching tick.
        for singleton in [
            points[0],
            points[count / 2],
            points[count - 1],
            (-1, tempo(0)),
        ] {
            let after = project(&[singleton]);
            check(&before, &after);
            check(&after, &before);
        }
    }
}

#[test]
fn tempo_diffs_keep_signed_tick_order_and_exact_rationals_at_extremes() {
    let before = project(&[
        (i64::MIN, Tempo::rational(u32::MAX, u32::MAX - 1).unwrap()),
        (-1, Tempo::rational(1, u32::MAX).unwrap()),
        (0, tempo(0)),
        (10, tempo(1)),
        (i64::MAX, tempo(2)),
    ]);
    let after = project(&[
        (i64::MIN, Tempo::rational(1, u32::MAX).unwrap()),
        (-1, tempo(3)),
        (1, tempo(4)),
        (2, tempo(5)),
        (10, tempo(1)),
    ]);
    check(&before, &after);
    check(&after, &before);
    for at in [i64::MIN, i64::MAX] {
        let singleton = project(&[(at, tempo(0))]);
        check(&before, &singleton);
        check(&singleton, &before);
    }
}
