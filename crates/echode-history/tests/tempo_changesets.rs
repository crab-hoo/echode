//! Tempo replay expectations independent of the changeset field table.

use std::collections::BTreeMap;

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, ObjectKey, ObjectKind, Project, Property, Tempo,
    TempoMap, TempoPoint, Tick, Value,
};
use proptest::prelude::*;

fn tempo(numerator: u32, denominator: u32) -> Tempo {
    Tempo::rational(numerator, denominator).unwrap()
}

fn project(points: &[(i64, Tempo)]) -> Project {
    Project::try_assemble(
        Project::new().meter(),
        TempoMap::try_from_points(
            points
                .iter()
                .map(|(at, tempo)| TempoPoint {
                    at: Tick::new(*at),
                    tempo: *tempo,
                })
                .collect(),
        )
        .unwrap(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn fields(value: Tempo) -> Vec<Field> {
    vec![Field {
        property: Property::Tempo,
        value: Value::Tempo(value),
    }]
}

fn create(at: i64, value: Tempo) -> Change {
    Change::Create {
        kind: ObjectKind::TempoPoint,
        id: ObjectKey::Tempo(Tick::new(at)),
        fields: fields(value),
    }
}

fn delete(at: i64, value: Tempo) -> Change {
    Change::Delete {
        kind: ObjectKind::TempoPoint,
        id: ObjectKey::Tempo(Tick::new(at)),
        fields: fields(value),
    }
}

fn update(at: i64, before: Tempo, after: Tempo) -> Change {
    Change::Update {
        id: ObjectKey::Tempo(Tick::new(at)),
        property: Property::Tempo,
        before: Value::Tempo(before),
        after: Value::Tempo(after),
    }
}

fn changes(items: impl IntoIterator<Item = Change>) -> ChangeSet {
    let mut result = ChangeSet::new();
    for item in items {
        result.push(item);
    }
    result
}

fn journal_roundtrip(changes: &ChangeSet) -> ChangeSet {
    let line = JournalLine {
        seq: 1,
        op: JournalOp::Edit,
        actions: Vec::new(),
        changes: changes.clone(),
    };
    let decoded = decode_line(&encode_line(&line)).unwrap();
    assert_eq!(decoded, line);
    decoded.changes
}

fn check_roundtrip(before: &Project, after: &Project) {
    let points = |project: &Project| -> BTreeMap<_, _> {
        project
            .tempo()
            .points()
            .iter()
            .map(|point| (point.at.get(), point.tempo))
            .collect()
    };
    let left = points(before);
    let right = points(after);
    let mut expected = Vec::new();
    for (&at, &value) in &right {
        if !left.contains_key(&at) {
            expected.push(create(at, value));
        }
    }
    for (&at, &value) in &right {
        if let Some(&old) = left.get(&at)
            && old != value
        {
            expected.push(update(at, old, value));
        }
    }
    for (&at, &value) in &left {
        if !right.contains_key(&at) {
            expected.push(delete(at, value));
        }
    }
    let diff = before.diff(after);
    assert_eq!(
        diff.changes(),
        expected,
        "tempo fields or tick order changed"
    );
    assert_eq!(
        before.apply_changeset(&journal_roundtrip(&diff)).unwrap(),
        *after
    );
    assert_eq!(
        after
            .apply_changeset(&journal_roundtrip(&diff.invert()))
            .unwrap(),
        *before
    );
    assert_eq!(diff.invert().invert(), diff);
    assert!(after.diff(after).is_empty());
}

fn rational_tempo() -> impl Strategy<Value = Tempo> {
    (1u32..=u32::MAX, 1u32..=u32::MAX)
        .prop_filter_map("valid rational tempo", |(n, d)| Tempo::rational(n, d))
}

proptest! {
    // Map lengths and case counts bound this regression, not product tempo maps.
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_tempo_maps_keep_exact_fields_and_inverse(
        before in prop::collection::btree_map(any::<i64>(), rational_tempo(), 1..9),
        after in prop::collection::btree_map(any::<i64>(), rational_tempo(), 1..9),
    ) {
        check_roundtrip(
            &project(&before.into_iter().collect::<Vec<_>>()),
            &project(&after.into_iter().collect::<Vec<_>>()),
        );
    }

    #[test]
    fn replacing_a_point_preserves_the_rational_value(
        at in any::<i64>(), before in rational_tempo(), after in rational_tempo(),
    ) {
        check_roundtrip(&project(&[(at, before)]), &project(&[(at, after)]));
    }
}

#[test]
fn creates_updates_and_deletes_are_each_sorted_by_signed_tick() {
    let before = project(&[
        (i64::MAX, tempo(999, 1)),
        (0, tempo(120, 1)),
        (10, tempo(600, 14)),
        (-1, tempo(1, u32::MAX)),
        (i64::MIN, tempo(u32::MAX, u32::MAX - 1)),
    ]);
    let after = project(&[
        (2, tempo(300, 7)),
        (-1, tempo(1, 1)),
        (10, tempo(300, 7)),
        (1, tempo(999, 1)),
        (i64::MIN, tempo(1, u32::MAX)),
    ]);
    check_roundtrip(&before, &after);
    check_roundtrip(&after, &before);
}

#[test]
fn tempo_wire_names_and_rational_components_stay_compatible() {
    let text = r#"{"seq":1,"kind":"edit","changes":[{"op":"create","kind":"tempo","id":"tempo:-7","fields":[{"property":"tempo","value":{"type":"tempo","numerator":300,"denominator":7}}]}]}"#;
    let line = decode_line(text).unwrap();
    assert_eq!(line.changes, changes([create(-7, tempo(300, 7))]));
    assert_eq!(encode_line(&line), text);
    let base = Project::new();
    assert_eq!(
        base.apply_changeset(&line.changes).unwrap(),
        base.with_tempo(Tick::new(-7), tempo(300, 7))
    );
}

fn rejected(base: &Project, items: impl IntoIterator<Item = Change>) {
    let snapshot = base.clone();
    assert_eq!(
        base.apply_changeset(&changes(items)),
        Err(ModelError::InvalidChange)
    );
    assert_eq!(*base, snapshot, "rejected tempo changes mutated the source");
}

#[test]
fn invalid_fields_keys_and_missing_points_are_rejected_atomically() {
    let base = Project::new();
    let id = ObjectKey::Tempo(Tick::new(-7));
    for invalid in [
        Vec::new(),
        vec![Field {
            property: Property::Tempo,
            value: Value::Tick(Tick::new(90)),
        }],
        vec![Field {
            property: Property::Start,
            value: Value::Tempo(tempo(90, 1)),
        }],
    ] {
        rejected(
            &base,
            [Change::Create {
                kind: ObjectKind::TempoPoint,
                id,
                fields: invalid,
            }],
        );
    }
    rejected(&base, [create(0, tempo(90, 1))]);
    rejected(&base, [create(-7, tempo(90, 1)), create(-7, tempo(100, 1))]);
    rejected(&base, [update(-7, tempo(120, 1), tempo(90, 1))]);
    rejected(&base, [delete(-7, tempo(120, 1))]);
    rejected(&base, [delete(0, tempo(120, 1))]);
    for key in [ObjectKey::Project, ObjectKey::Id(Id::generate())] {
        rejected(
            &base,
            [Change::Create {
                kind: ObjectKind::TempoPoint,
                id: key,
                fields: fields(tempo(90, 1)),
            }],
        );
        rejected(
            &base,
            [Change::Delete {
                kind: ObjectKind::TempoPoint,
                id: key,
                fields: fields(tempo(90, 1)),
            }],
        );
        rejected(
            &base,
            [Change::Update {
                id: key,
                property: Property::Tempo,
                before: Value::Tempo(tempo(120, 1)),
                after: Value::Tempo(tempo(90, 1)),
            }],
        );
    }
    rejected(
        &base,
        [Change::Create {
            kind: ObjectKind::Track,
            id,
            fields: fields(tempo(90, 1)),
        }],
    );
    rejected(
        &base,
        [Change::Delete {
            kind: ObjectKind::Track,
            id: ObjectKey::Tempo(Tick::new(0)),
            fields: Vec::new(),
        }],
    );
    for (property, value) in [
        (Property::Tempo, Value::Tick(Tick::new(90))),
        (Property::Start, Value::Tempo(tempo(90, 1))),
        (Property::Meter, Value::Meter(base.meter())),
    ] {
        rejected(
            &base,
            [
                update(0, tempo(120, 1), tempo(100, 1)),
                Change::Update {
                    id: ObjectKey::Tempo(Tick::new(0)),
                    property,
                    before: value.clone(),
                    after: value,
                },
            ],
        );
    }
}

#[test]
fn legacy_creation_and_replacement_semantics_are_preserved() {
    let base = Project::new();
    let mut legacy = vec![Field {
        property: Property::Tempo,
        value: Value::Text("ignored wrong type".into()),
    }];
    legacy.extend(fields(tempo(300, 7)));
    legacy.extend(fields(tempo(999, 1)));
    legacy.push(Field {
        property: Property::Start,
        value: Value::Tick(Tick::new(100)),
    });
    let created = changes([Change::Create {
        kind: ObjectKind::TempoPoint,
        id: ObjectKey::Tempo(Tick::new(-7)),
        fields: legacy,
    }]);
    assert_eq!(
        base.apply_changeset(&journal_roundtrip(&created)).unwrap(),
        base.with_tempo(Tick::new(-7), tempo(300, 7)),
        "creation must keep the first correctly typed tempo field and ignore extra fields"
    );
    let replaced = changes([
        Change::Delete {
            kind: ObjectKind::TempoPoint,
            id: ObjectKey::Tempo(Tick::new(0)),
            fields: Vec::new(),
        },
        create(0, tempo(300, 7)),
        update(0, tempo(1, 1), tempo(999, 1)),
    ]);
    assert_eq!(
        base.apply_changeset(&journal_roundtrip(&replaced)).unwrap(),
        base.with_tempo(Tick::new(0), tempo(999, 1)),
        "delete payload and update before are not replay preconditions"
    );
}
