//! Property-level parameter replay, independent of the changeset field table.
//!
//! Arbitrary float bits intentionally exercise the low-level changeset API, not
//! valid mixer settings. DocumentHost still validates invariants before publishing.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, ObjectKey, ObjectKind, Project, Property, Value,
};
use proptest::prelude::*;

const FIELDS: [Property; 4] = [
    Property::Minimum,
    Property::Maximum,
    Property::Default,
    Property::Value,
];

fn update(project: &Project, id: Id, bits: [u32; 4]) -> Project {
    let mut changes = ChangeSet::new();
    for (property, after) in FIELDS.into_iter().zip(bits) {
        changes.push(Change::Update {
            id: ObjectKey::Id(id),
            property,
            before: Value::Bits(0),
            after: Value::Bits(after),
        });
    }
    project.apply_changeset(&changes).unwrap()
}

fn bits(project: &Project) -> [u32; 4] {
    let parameter = &project.tracks()[0].devices[0].parameters[0];
    [
        parameter.min.to_bits(),
        parameter.max.to_bits(),
        parameter.default.to_bits(),
        parameter.value.to_bits(),
    ]
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

fn check_roundtrip(before: [u32; 4], after: [u32; 4]) {
    let empty = Project::new();
    let (base, _) = empty.add_track("parameter fields");
    let id = base.tracks()[0].devices[0].parameters[0].id;
    let before_project = update(&base, id, before);
    let after_project = update(&base, id, after);
    assert_eq!(
        bits(&before_project),
        before,
        "parameter writes crossed fields"
    );
    assert_eq!(
        bits(&after_project),
        after,
        "parameter writes lost float bits"
    );

    let changes = before_project.diff(&after_project);
    let expected: Vec<_> = FIELDS
        .into_iter()
        .zip(before.into_iter().zip(after))
        .filter(|(_, (left, right))| left != right)
        .map(|(property, (left, right))| Change::Update {
            id: ObjectKey::Id(id),
            property,
            before: Value::Bits(left),
            after: Value::Bits(right),
        })
        .collect();
    assert_eq!(
        changes.changes(),
        expected,
        "parameter diff changed its wire fields"
    );
    assert_eq!(replay(&before_project, &changes), after_project);
    assert_eq!(replay(&after_project, &changes.invert()), before_project);
    assert_eq!(changes.invert().invert(), changes);

    let created = empty.diff(&after_project);
    let fields = created
        .changes()
        .iter()
        .find_map(|change| match change {
            Change::Create {
                kind: ObjectKind::Parameter,
                id: ObjectKey::Id(found),
                fields,
            } if *found == id => Some(fields),
            _ => None,
        })
        .unwrap();
    let mut expected_fields = vec![
        Field {
            property: Property::Owner,
            value: Value::Id(base.tracks()[0].devices[0].id),
        },
        Field {
            property: Property::Key,
            value: Value::Text("gain".into()),
        },
    ];
    expected_fields.extend(
        FIELDS
            .into_iter()
            .zip(after)
            .map(|(property, value)| Field {
                property,
                value: Value::Bits(value),
            }),
    );
    assert_eq!(
        *fields, expected_fields,
        "parameter creation changed its wire fields"
    );
    assert_eq!(replay(&empty, &created), after_project);
    assert_eq!(replay(&after_project, &created.invert()), empty);
    let deleted = after_project.diff(&empty);
    assert_eq!(replay(&after_project, &deleted), empty);
    assert_eq!(replay(&empty, &deleted.invert()), after_project);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn parameter_changesets_preserve_arbitrary_float_bits(
        before in any::<[u32; 4]>(), after in any::<[u32; 4]>()
    ) {
        check_roundtrip(before, after);
    }
}

#[test]
fn parameter_changesets_keep_signed_zero_subnormals_infinities_and_nan_payloads() {
    let patterns = [
        0,
        0x8000_0000,
        1,
        0x8000_0001,
        0x7f7f_ffff,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0123,
    ];
    for (index, pattern) in patterns.into_iter().enumerate() {
        for field in 0..4 {
            let before = [0x3f80_0000; 4];
            let mut after = before;
            after[field] = pattern;
            after[(field + 1) % 4] = patterns[(index + 1) % patterns.len()];
            check_roundtrip(before, after);
            check_roundtrip(after, after);
        }
    }
}

#[test]
fn parameter_changesets_reject_missing_fields_wrong_variants_and_immutable_keys() {
    let empty = Project::new();
    let (base, _) = empty.add_track("invalid parameter fields");
    let id = base.tracks()[0].devices[0].parameters[0].id;
    let created = empty.diff(&base);
    for property in [
        Property::Owner,
        Property::Key,
        Property::Minimum,
        Property::Maximum,
        Property::Default,
        Property::Value,
    ] {
        for wrong_type in [false, true] {
            let mut changed = ChangeSet::new();
            for change in created.changes() {
                let mut change = change.clone();
                if let Change::Create {
                    kind: ObjectKind::Parameter,
                    id: ObjectKey::Id(found),
                    fields,
                } = &mut change
                    && *found == id
                {
                    if wrong_type {
                        fields
                            .iter_mut()
                            .find(|field| field.property == property)
                            .unwrap()
                            .value = Value::Byte(0);
                    } else {
                        fields.retain(|field| field.property != property);
                    }
                }
                changed.push(change);
            }
            assert_eq!(
                empty.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "{property:?}, wrong_type={wrong_type}"
            );
        }
    }
    for (target, property, after) in FIELDS
        .into_iter()
        .map(|property| (id, property, Value::Byte(0)))
        .chain([
            (Id::generate(), Property::Value, Value::Bits(0)),
            (id, Property::Owner, Value::Id(Id::generate())),
            (id, Property::Key, Value::Text("pan".into())),
        ])
    {
        let mut changed = ChangeSet::new();
        changed.push(Change::Update {
            id: ObjectKey::Id(target),
            property,
            before: Value::Bits(0),
            after,
        });
        assert_eq!(
            base.apply_changeset(&changed),
            Err(ModelError::InvalidChange),
            "{property:?}"
        );
    }
    base.check_invariants().unwrap();
}
