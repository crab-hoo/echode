//! Track replay expectations independent of the changeset field table.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, NoteDraft, ObjectKey, ObjectKind, Project, Property,
    Tick, Track, Value,
};
use proptest::prelude::*;

fn fixture() -> (Project, Id, [Id; 2], [String; 2]) {
    let (project, left) = Project::new().add_track("left folder");
    let (project, right) = project.add_track("right folder");
    let (project, target) = project.add_track("target");
    let first = track(&project, target).order.clone();
    let (extra, last) = project.add_track("order key source only");
    let second = track(&extra, last).order.clone();
    let project = project.set_parent(target, Some(left)).unwrap();
    (project, target, [left, right], [first, second])
}

fn track(project: &Project, id: Id) -> &Track {
    project
        .tracks()
        .iter()
        .find(|track| track.id == id)
        .unwrap()
}

fn field(property: Property, value: Value) -> Field {
    Field { property, value }
}

fn fields(track: &Track) -> Vec<Field> {
    vec![
        field(Property::Name, Value::Text(track.name.clone())),
        field(Property::Parent, Value::Parent(track.parent)),
        field(Property::Order, Value::Text(track.order.clone())),
        field(
            Property::ClipOrder,
            Value::Ids(track.clips.iter().map(|clip| clip.id).collect()),
        ),
        field(
            Property::DeviceOrder,
            Value::Ids(track.devices.iter().map(|device| device.id).collect()),
        ),
    ]
}

fn update(id: Id, property: Property, before: Value, after: Value) -> Change {
    Change::Update {
        id: ObjectKey::Id(id),
        property,
        before,
        after,
    }
}

fn changes(items: impl IntoIterator<Item = Change>) -> ChangeSet {
    let mut changes = ChangeSet::new();
    for item in items {
        changes.push(item);
    }
    changes
}

fn write(project: &Project, value: &Track) -> Project {
    project
        .apply_changeset(&changes(fields(value).into_iter().map(|field| {
            update(value.id, field.property, field.value.clone(), field.value)
        })))
        .unwrap()
}

fn replay(project: &Project, changes: &ChangeSet) -> Project {
    let line = JournalLine {
        seq: 1,
        op: JournalOp::Edit,
        actions: Vec::new(),
        changes: changes.clone(),
    };
    let decoded = decode_line(&encode_line(&line)).unwrap();
    assert_eq!(decoded, line);
    project.apply_changeset(&decoded.changes).unwrap()
}

fn check_roundtrip(left: &Project, right: &Project, target: Id) {
    left.check_invariants().unwrap();
    right.check_invariants().unwrap();
    let diff = left.diff(right);
    let expected: Vec<_> = fields(track(left, target))
        .into_iter()
        .zip(fields(track(right, target)))
        .filter(|(left, right)| left.value != right.value)
        .map(|(left, right)| update(target, left.property, left.value, right.value))
        .collect();
    let actual: Vec<_> = diff
        .changes()
        .iter()
        .filter(|change| matches!(change, Change::Update { id: ObjectKey::Id(id), .. } if *id == target))
        .cloned()
        .collect();
    assert_eq!(actual, expected, "track diff changed its wire fields");
    assert_eq!(replay(left, &diff), *right);
    assert_eq!(replay(right, &diff.invert()), *left);
    assert_eq!(diff.invert().invert(), diff);

    let empty = Project::new();
    for (source, destination, create) in [(&empty, right, true), (right, &empty, false)] {
        let diff = source.diff(destination);
        let actual = diff
            .changes()
            .iter()
            .find_map(|change| match change {
                Change::Create {
                    kind: ObjectKind::Track,
                    id: ObjectKey::Id(id),
                    fields,
                } if create && *id == target => Some(fields),
                Change::Delete {
                    kind: ObjectKind::Track,
                    id: ObjectKey::Id(id),
                    fields,
                } if !create && *id == target => Some(fields),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            *actual,
            fields(track(right, target)),
            "track creation/deletion changed its wire fields"
        );
        assert_eq!(replay(source, &diff), *destination);
        assert_eq!(replay(destination, &diff.invert()), *source);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn track_changesets_preserve_names_parents_and_fractional_keys(
        before_name in ".{0,32}", after_name in ".{0,32}",
        before_parent in 0usize..3, after_parent in 0usize..3,
        before_order in any::<bool>(), after_order in any::<bool>(),
    ) {
        let (base, target, parents, orders) = fixture();
        let before = Track {
            name: before_name,
            parent: parents.get(before_parent).copied(),
            order: orders[usize::from(before_order)].clone(),
            ..track(&base, target).clone()
        };
        let after = Track {
            name: after_name,
            parent: parents.get(after_parent).copied(),
            order: orders[usize::from(after_order)].clone(),
            ..before.clone()
        };
        let left = write(&base, &before);
        let right = write(&base, &after);
        assert_eq!(*track(&left, target), before, "track writes crossed fields");
        assert_eq!(*track(&right, target), after, "track writes lost stored values");
        check_roundtrip(&left, &right, target);
    }
}

#[test]
fn track_changesets_cover_all_scalar_combinations_and_child_orders() {
    let (base, target, parents, orders) = fixture();
    let before = Track {
        name: "old\0\n日本語".into(),
        parent: Some(parents[0]),
        order: orders[0].clone(),
        ..track(&base, target).clone()
    };
    let left = write(&base, &before);
    // This mask enumerates the three scalar fields, not a product limit.
    for mask in 0..8 {
        let after = Track {
            name: if mask & 1 != 0 {
                "new 🎹\r\n".into()
            } else {
                before.name.clone()
            },
            parent: if mask & 2 != 0 {
                Some(parents[1])
            } else {
                before.parent
            },
            order: orders[usize::from(mask & 4 != 0)].clone(),
            ..before.clone()
        };
        let right = write(&base, &after);
        check_roundtrip(&left, &right, target);
        assert_eq!(left.diff(&right).is_empty(), mask == 0);
    }
    let right = left
        .insert_note(
            target,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(100),
            },
        )
        .unwrap()
        .0
        .add_clap_device(target, Id::generate(), "org.example.fields")
        .unwrap();
    let after = Track {
        name: "new name".into(),
        parent: Some(parents[1]),
        order: orders[1].clone(),
        ..track(&right, target).clone()
    };
    let right = write(&right, &after);
    check_roundtrip(&left, &right, target);
    assert_eq!(
        left.diff(&right)
            .changes()
            .iter()
            .filter_map(|change| match change {
                Change::Update {
                    id: ObjectKey::Id(id),
                    property,
                    ..
                } if *id == target => Some(*property),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [
            Property::Name,
            Property::Parent,
            Property::Order,
            Property::ClipOrder,
            Property::DeviceOrder
        ],
        "child orders must follow track scalars",
    );
}

#[test]
fn track_changesets_reject_missing_fields_wrong_types_and_unknown_targets() {
    let (base, target, _, _) = fixture();
    let empty = Project::new();
    let created = empty.diff(&base);
    for field in fields(track(&base, target)) {
        for wrong_type in [false, true] {
            let changed = changes(created.changes().iter().cloned().map(|mut change| {
                if let Change::Create {
                    kind: ObjectKind::Track,
                    id: ObjectKey::Id(id),
                    fields,
                } = &mut change
                    && *id == target
                {
                    if wrong_type {
                        fields
                            .iter_mut()
                            .find(|item| item.property == field.property)
                            .unwrap()
                            .value = Value::Tick(Tick::new(0));
                    } else {
                        fields.retain(|item| item.property != field.property);
                    }
                }
                change
            }));
            assert_eq!(
                empty.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "track creation {:?}, wrong_type={wrong_type}",
                field.property
            );
        }
        for (id, value) in [
            (target, Value::Tick(Tick::new(0))),
            (Id::from_raw(0), field.value.clone()),
        ] {
            let changed = changes([update(id, field.property, field.value.clone(), value)]);
            assert_eq!(
                base.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "track update {:?}, target={id}",
                field.property
            );
        }
    }
    let unsupported = changes([update(
        target,
        Property::Key,
        Value::Text("old".into()),
        Value::Text("new".into()),
    )]);
    assert_eq!(
        base.apply_changeset(&unsupported),
        Err(ModelError::InvalidChange)
    );
    let legacy_lane = changes([update(
        target,
        Property::Lane,
        Value::Text("timeline".into()),
        Value::Text("timeline".into()),
    )]);
    assert_eq!(replay(&base, &legacy_lane), base);
}

#[test]
fn track_changesets_keep_hierarchy_and_child_resolution_checks() {
    let (base, target, parents, _) = fixture();
    for parent in [Some(target), Some(Id::from_raw(0))] {
        let invalid = changes([update(
            target,
            Property::Parent,
            Value::Parent(Some(parents[0])),
            Value::Parent(parent),
        )]);
        assert_eq!(
            base.apply_changeset(&invalid),
            Err(ModelError::BadOrder),
            "invalid parent {parent:?}"
        );
    }
    let cycle = changes([update(
        parents[0],
        Property::Parent,
        Value::Parent(None),
        Value::Parent(Some(target)),
    )]);
    assert_eq!(base.apply_changeset(&cycle), Err(ModelError::BadOrder));
    for key in [
        String::new(),
        "not a fractional index".into(),
        track(&base, parents[1]).order.clone(),
    ] {
        let invalid = changes([update(
            parents[0],
            Property::Order,
            Value::Text(track(&base, parents[0]).order.clone()),
            Value::Text(key.clone()),
        )]);
        assert_eq!(
            base.apply_changeset(&invalid),
            Err(ModelError::BadOrder),
            "invalid sibling key {key:?}"
        );
    }
    let with_note = base
        .insert_note(
            target,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(100),
            },
        )
        .unwrap()
        .0;
    for property in [Property::ClipOrder, Property::DeviceOrder] {
        let stored = fields(track(&with_note, target))
            .into_iter()
            .find(|field| field.property == property)
            .unwrap()
            .value;
        for ids in [vec![], vec![Id::from_raw(0)]] {
            let invalid = changes([update(target, property, stored.clone(), Value::Ids(ids))]);
            assert_eq!(
                with_note.apply_changeset(&invalid),
                Err(ModelError::InvalidChange),
                "unresolved child order {property:?}"
            );
        }
    }
    assert_eq!(
        base.check_invariants(),
        Ok(()),
        "refused replay changed its source"
    );
}
