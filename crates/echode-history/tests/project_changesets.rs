//! Project replay expectations independent of the changeset field table.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, ClipKind, Id, Meter, ModelError, NoteDraft, ObjectKey, ObjectKind, Project,
    Property, Tempo, Tick, Value,
};
use proptest::prelude::*;

fn changes(items: impl IntoIterator<Item = Change>) -> ChangeSet {
    let mut result = ChangeSet::new();
    for item in items {
        result.push(item);
    }
    result
}

fn update(id: ObjectKey, property: Property, before: Value, after: Value) -> Change {
    Change::Update {
        id,
        property,
        before,
        after,
    }
}

fn meter_change(before: Meter, after: Meter) -> Change {
    update(
        ObjectKey::Project,
        Property::Meter,
        Value::Meter(before),
        Value::Meter(after),
    )
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

fn with_meter(project: &Project, meter: Meter) -> Project {
    // Assemble saved parts, not a meter action: replay must not refit clip windows.
    Project::try_assemble(
        meter,
        project.tempo().clone(),
        project
            .contents()
            .iter()
            .map(|part| (**part).clone())
            .collect(),
        project
            .tracks()
            .iter()
            .map(|part| (**part).clone())
            .collect(),
    )
    .unwrap()
}

fn populated() -> Project {
    let (project, root) = Project::new().add_track("folder 日本語");
    let (project, child) = project.add_child(root, "child 🎹").unwrap();
    let (project, _) = project.add_track("other root");
    let (project, _) = project
        .insert_note(
            child,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(1),
                length: Tick::new(7),
            },
        )
        .unwrap();
    let clip = &project
        .tracks()
        .iter()
        .find(|track| track.id == child)
        .unwrap()
        .clips[0];
    let project = project
        .apply_changeset(&changes([update(
            ObjectKey::Id(clip.id),
            Property::Kind,
            Value::Kind(clip.kind),
            Value::Kind(ClipKind::Fixed),
        )]))
        .unwrap();
    project.with_tempo(Tick::new(-7), Tempo::rational(300, 7).unwrap())
}

fn check_meter_roundtrip(base: &Project, before: Meter, after: Meter) {
    let left = with_meter(base, before);
    let right = with_meter(base, after);
    let expected = if before == after {
        Vec::new()
    } else {
        vec![meter_change(before, after)]
    };
    let diff = left.diff(&right);
    assert_eq!(diff.changes(), expected, "project fields changed");
    assert_eq!(
        left.apply_changeset(&journal_roundtrip(&diff)).unwrap(),
        right
    );
    assert_eq!(
        right
            .apply_changeset(&journal_roundtrip(&diff.invert()))
            .unwrap(),
        left
    );
    assert_eq!(diff.invert().invert(), diff);
    assert!(right.diff(&right).is_empty());
}

fn signature() -> impl Strategy<Value = Meter> {
    // Each denominator divides the tick grid; the numerator need not be small.
    (
        1u32..=u32::MAX,
        prop::sample::select(vec![1, 2, 3, 4, 7, 8, 9, 25, 64, 128, 2_822_400]),
    )
        .prop_map(|(n, d)| Meter::try_new(n, d).unwrap())
}

proptest! {
    // Case counts bound this regression, not the product's meter range.
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_meters_preserve_exact_components_and_other_project_parts(
        before in signature(), after in signature(), filled in any::<bool>(),
    ) {
        let base = if filled { populated() } else { Project::new() };
        check_meter_roundtrip(&base, before, after);
    }
}

#[test]
fn equivalent_bar_lengths_do_not_normalize_the_signature() {
    let base = populated();
    for (before, after) in [
        ((6, 8), (3, 4)),
        ((u32::MAX, u32::MAX), (1, 1)),
        ((1, 2_822_400), (u32::MAX, 1)),
    ] {
        check_meter_roundtrip(
            &base,
            Meter::try_new(before.0, before.1).unwrap(),
            Meter::try_new(after.0, after.1).unwrap(),
        );
    }
}

#[test]
fn meter_updates_keep_their_position_between_tempo_and_child_updates() {
    let before = populated();
    let next_meter = Meter::try_new(7, 8).unwrap();
    let gain = before.tracks()[0].devices[0].parameters[0].id;
    let after = with_meter(&before, next_meter)
        .with_tempo(Tick::new(0), Tempo::quarters_per_minute(90).unwrap())
        .set_parameter(gain, 0.5)
        .unwrap();
    let diff = before.diff(&after);
    assert_eq!(
        diff.changes(),
        [
            update(
                ObjectKey::Tempo(Tick::new(0)),
                Property::Tempo,
                Value::Tempo(Tempo::quarters_per_minute(120).unwrap()),
                Value::Tempo(Tempo::quarters_per_minute(90).unwrap()),
            ),
            meter_change(before.meter(), next_meter),
            update(
                ObjectKey::Id(gain),
                Property::Value,
                Value::Bits(1.0f32.to_bits()),
                Value::Bits(0.5f32.to_bits()),
            ),
        ]
    );
    assert_eq!(
        before.apply_changeset(&journal_roundtrip(&diff)).unwrap(),
        after
    );
    assert_eq!(after.apply_changeset(&diff.invert()).unwrap(), before);
}

#[test]
fn project_wire_names_and_unreduced_meter_components_stay_compatible() {
    let text = r#"{"seq":1,"kind":"edit","changes":[{"op":"update","id":"project","property":"meter","before":{"type":"meter","numerator":4,"denominator":4},"after":{"type":"meter","numerator":6,"denominator":8}},{"op":"update","id":"project","property":"track_order","before":{"type":"ids","ids":[]},"after":{"type":"ids","ids":[]}}]}"#;
    let line = decode_line(text).unwrap();
    let base = Project::new();
    let meter = Meter::try_new(6, 8).unwrap();
    assert_eq!(
        line.changes,
        changes([
            meter_change(base.meter(), meter),
            update(
                ObjectKey::Project,
                Property::TrackOrder,
                Value::Ids(vec![]),
                Value::Ids(vec![])
            ),
        ])
    );
    assert_eq!(encode_line(&line), text);
    assert_eq!(
        base.apply_changeset(&line.changes).unwrap(),
        with_meter(&base, meter)
    );
    for (n, d) in [(0, 4), (4, 0), (1, 11)] {
        let invalid = text.replace(
            "\"numerator\":6,\"denominator\":8",
            &format!("\"numerator\":{n},\"denominator\":{d}"),
        );
        assert!(
            decode_line(&invalid).is_err(),
            "accepted invalid signature {n}/{d}"
        );
    }
}

fn rejected(base: &Project, invalid: Change) {
    let snapshot = base.clone();
    assert_eq!(
        base.apply_changeset(&changes([
            meter_change(base.meter(), Meter::try_new(7, 8).unwrap()),
            invalid,
        ])),
        Err(ModelError::InvalidChange)
    );
    assert_eq!(
        *base, snapshot,
        "rejected project changes mutated the source"
    );
}

#[test]
fn project_updates_reject_wrong_value_types_properties_and_object_keys() {
    let base = populated();
    let id = base.tracks()[0].id;
    let values = [
        Value::Tempo(Tempo::quarters_per_minute(90).unwrap()),
        Value::Tick(Tick::new(7)),
        Value::Text("7/8".to_string()),
        Value::Id(id),
        Value::Ids(vec![id]),
        Value::Parent(None),
        Value::Audio(None),
        Value::Kind(ClipKind::Fixed),
        Value::Byte(7),
        Value::U16(7),
        Value::Gain(1.0f32.to_bits()),
        Value::Bits(1.0f32.to_bits()),
        Value::Flicks(7),
        Value::ParamIds(vec![]),
        Value::OrderStep {
            index: 0,
            id,
            present: true,
        },
    ];
    for value in values {
        rejected(
            &base,
            update(
                ObjectKey::Project,
                Property::Meter,
                Value::Meter(base.meter()),
                value,
            ),
        );
    }
    for (property, value) in [
        (Property::Name, Value::Text("project".to_string())),
        (
            Property::Tempo,
            Value::Tempo(Tempo::quarters_per_minute(90).unwrap()),
        ),
        (Property::Lane, Value::Text("legacy".to_string())),
        (Property::TrackOrder, Value::Meter(base.meter())),
        (
            Property::TrackOrder,
            Value::OrderStep {
                index: 0,
                id,
                present: true,
            },
        ),
    ] {
        rejected(
            &base,
            update(ObjectKey::Project, property, value.clone(), value),
        );
    }
    for key in [
        ObjectKey::Id(id),
        ObjectKey::Id(Id::from_raw(1)),
        ObjectKey::Tempo(Tick::new(0)),
    ] {
        for (property, value) in [
            (Property::Meter, Value::Meter(base.meter())),
            (Property::TrackOrder, Value::Ids(vec![id])),
        ] {
            rejected(&base, update(key, property, value.clone(), value));
        }
    }
    for kind in [
        ObjectKind::Track,
        ObjectKind::Clip,
        ObjectKind::Note,
        ObjectKind::TempoPoint,
        ObjectKind::Content,
        ObjectKind::Device,
        ObjectKind::Parameter,
    ] {
        rejected(
            &base,
            Change::Create {
                kind,
                id: ObjectKey::Project,
                fields: vec![],
            },
        );
        rejected(
            &base,
            Change::Delete {
                kind,
                id: ObjectKey::Project,
                fields: vec![],
            },
        );
    }
}

#[test]
fn replay_ignores_before_and_legacy_track_order_without_running_meter_actions() {
    let base = populated();
    let meter = Meter::try_new(7, 8).unwrap();
    assert_eq!(base.with_meter(meter), Err(ModelError::InvalidMeter));
    let ids = vec![
        base.tracks()[2].id,
        Id::from_raw(1),
        base.tracks()[0].id,
        base.tracks()[0].id,
    ];
    let legacy = changes([update(
        ObjectKey::Project,
        Property::TrackOrder,
        Value::Byte(0),
        Value::Ids(ids),
    )]);
    assert_eq!(
        base.apply_changeset(&journal_roundtrip(&legacy)).unwrap(),
        base
    );
    let edit = changes([
        update(
            ObjectKey::Project,
            Property::Meter,
            Value::Byte(0),
            Value::Meter(meter),
        ),
        legacy.changes()[0].clone(),
    ]);
    let after = base.apply_changeset(&journal_roundtrip(&edit)).unwrap();
    assert_eq!(after, with_meter(&base, meter));
    after.check_invariants().unwrap();
    assert!(after.diff(&base).changes().iter().all(|change| !matches!(
        change,
        Change::Update {
            property: Property::TrackOrder,
            ..
        }
    )));
}
