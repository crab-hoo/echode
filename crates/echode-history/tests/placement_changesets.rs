//! Placement replay expectations independent of the changeset field table.
//!
//! Raw integer and gain-bit coverage is separate from public model validation.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, ClipKind, Field, Id, ModelError, NoteDraft, ObjectKey, ObjectKind,
    Placement, Project, Property, Tick, Value,
};
use proptest::prelude::*;

fn fixture() -> (Project, Id, Id) {
    let mut project = Project::new();
    for name in ["placement fields", "other content"] {
        let (next, track) = project.add_track(name);
        project = next
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(100),
                },
            )
            .unwrap()
            .0;
    }
    let target = project.tracks()[0].clips[0].id;
    let other = project.tracks()[1].clips[0].content;
    let (project, track) = project.add_track("linked source content");
    let project = project
        .link_clip(target, track, Tick::new(100), Id::generate())
        .unwrap();
    (project, target, other)
}

fn clip(project: &Project, target: Id) -> (Id, &Placement) {
    project
        .tracks()
        .iter()
        .find_map(|track| {
            track
                .clips
                .iter()
                .find(|clip| clip.id == target)
                .map(|clip| (track.id, clip))
        })
        .unwrap()
}

fn field(property: Property, value: Value) -> Field {
    Field { property, value }
}

fn fields(owner: Id, clip: &Placement) -> Vec<Field> {
    vec![
        field(Property::Owner, Value::Id(owner)),
        field(Property::Content, Value::Id(clip.content)),
        field(Property::Start, Value::Tick(clip.start)),
        field(Property::Length, Value::Tick(clip.length)),
        field(Property::Offset, Value::Tick(clip.offset)),
        field(Property::Kind, Value::Kind(clip.kind)),
        field(Property::Gain, Value::Gain(clip.gain.to_bits())),
        field(Property::FadeIn, Value::Flicks(clip.fade_in)),
        field(Property::FadeOut, Value::Flicks(clip.fade_out)),
        field(Property::Lane, Value::Text("timeline".into())),
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

fn write(project: &Project, value: &Placement) -> Project {
    let owner = clip(project, value.id).0;
    project
        .apply_changeset(&changes(fields(owner, value).into_iter().map(|field| {
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
    let (before_owner, before) = clip(left, target);
    let (after_owner, after) = clip(right, target);
    let before_fields = fields(before_owner, before);
    let after_fields = fields(after_owner, after);
    let expected: Vec<_> = [
        Property::Owner,
        Property::Start,
        Property::Length,
        Property::Kind,
        Property::Offset,
        Property::Content,
        Property::Gain,
        Property::FadeIn,
        Property::FadeOut,
    ]
    .into_iter()
    .filter_map(|property| {
        let before = &before_fields
            .iter()
            .find(|f| f.property == property)
            .unwrap()
            .value;
        let after = &after_fields
            .iter()
            .find(|f| f.property == property)
            .unwrap()
            .value;
        (before != after).then(|| update(target, property, before.clone(), after.clone()))
    })
    .collect();
    let diff = left.diff(right);
    let actual: Vec<_> = diff
        .changes()
        .iter()
        .filter(|change| matches!(change, Change::Update { id: ObjectKey::Id(id), .. } if *id == target))
        .cloned()
        .collect();
    assert_eq!(actual, expected, "placement diff changed its wire fields");
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
                    kind: ObjectKind::Clip,
                    id: ObjectKey::Id(id),
                    fields,
                } if create && *id == target => Some(fields),
                Change::Delete {
                    kind: ObjectKind::Clip,
                    id: ObjectKey::Id(id),
                    fields,
                } if !create && *id == target => Some(fields),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            *actual, after_fields,
            "placement creation/deletion changed its wire fields"
        );
        assert_eq!(replay(source, &diff), *destination);
        assert_eq!(replay(destination, &diff.invert()), *source);
    }
}

type StoredValues = (i64, i64, i64, bool, u32, i64, i64, bool);

fn stored_values() -> impl Strategy<Value = StoredValues> {
    (
        any::<i64>(),
        any::<i64>(),
        any::<i64>(),
        any::<bool>(),
        any::<u32>(),
        any::<i64>(),
        any::<i64>(),
        any::<bool>(),
    )
}

fn with_values(base: &Placement, other: Id, values: StoredValues) -> Placement {
    let (start, length, offset, fixed, gain, fade_in, fade_out, switch_content) = values;
    Placement {
        start: Tick::new(start),
        length: Tick::new(length),
        offset: Tick::new(offset),
        kind: if fixed {
            ClipKind::Fixed
        } else {
            ClipKind::Auto
        },
        gain: f32::from_bits(gain),
        fade_in,
        fade_out,
        content: if switch_content { other } else { base.content },
        ..base.clone()
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn placement_changesets_preserve_stored_integers_and_gain_bits(
        before in stored_values(), after in stored_values(),
    ) {
        let (base, target, other) = fixture();
        let template = clip(&base, target).1;
        let before = with_values(template, other, before);
        let after = with_values(template, other, after);
        let left = write(&base, &before);
        let right = write(&base, &after);
        assert_eq!(clip(&left, target).1, &before, "placement writes crossed fields");
        assert_eq!(clip(&right, target).1, &after, "placement writes lost stored values");
        check_roundtrip(&left, &right, target);
    }
}

#[test]
fn placement_changesets_cover_all_field_combinations_and_float_edges() {
    let (base, target, other) = fixture();
    let (owner, before) = clip(&base, target);
    let alternate = with_values(
        before,
        other,
        (i64::MIN, i64::MAX, -17, true, 1, 1, i64::MAX, true),
    );
    // This mask enumerates eight stored fields, not a product limit.
    for mask in 0..256 {
        let mut after = fields(owner, before);
        for (index, field) in fields(owner, &alternate)
            .into_iter()
            .skip(1)
            .take(8)
            .enumerate()
        {
            if mask & (1 << index) != 0 {
                after[index + 1] = field;
            }
        }
        let right = base
            .apply_changeset(&changes(after.into_iter().map(|field| {
                update(target, field.property, field.value.clone(), field.value)
            })))
            .unwrap();
        check_roundtrip(&base, &right, target);
        assert_eq!(base.diff(&right).is_empty(), mask == 0);
    }
    for bits in [
        0,
        1,
        0x8000_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_1234,
        0x7fa0_4321,
        u32::MAX,
    ] {
        let right = write(
            &base,
            &Placement {
                gain: f32::from_bits(bits),
                ..before.clone()
            },
        );
        assert_eq!(clip(&right, target).1.gain.to_bits(), bits);
        check_roundtrip(&base, &right, target);
        let unchanged = write(&right, clip(&right, target).1);
        assert!(
            right.diff(&unchanged).is_empty(),
            "gain bits {bits:#x} changed without an edit"
        );
    }
}

#[test]
fn placement_changesets_reject_missing_fields_wrong_variants_and_unknown_targets() {
    let (base, target, _) = fixture();
    let (owner, value) = clip(&base, target);
    let empty = Project::new();
    let created = empty.diff(&base);
    for field in fields(owner, value)
        .into_iter()
        .filter(|field| field.property != Property::Lane)
    {
        for wrong_type in [false, true] {
            let changed = changes(created.changes().iter().cloned().map(|mut change| {
                if let Change::Create {
                    kind: ObjectKind::Clip,
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
                            .value = Value::Ids(vec![]);
                    } else {
                        fields.retain(|item| item.property != field.property);
                    }
                }
                change
            }));
            assert_eq!(
                empty.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "placement creation {:?}, wrong_type={wrong_type}",
                field.property
            );
        }
        for (id, value) in [
            (target, Value::Ids(vec![])),
            (Id::from_raw(0), field.value.clone()),
        ] {
            let changed = changes([update(id, field.property, field.value.clone(), value)]);
            assert_eq!(
                base.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "placement update {:?}, target={id}",
                field.property
            );
        }
    }
    let unknown = changes([update(
        target,
        Property::Key,
        Value::Text("old".into()),
        Value::Text("new".into()),
    )]);
    assert_eq!(
        base.apply_changeset(&unknown),
        Err(ModelError::InvalidChange)
    );
    let mut duplicate = created.clone();
    duplicate.push(
        created
            .changes()
            .iter()
            .find(|change| {
                matches!(change, Change::Create { kind: ObjectKind::Clip, id: ObjectKey::Id(id), .. } if *id == target)
            })
            .unwrap()
            .clone(),
    );
    assert_eq!(
        empty.apply_changeset(&duplicate),
        Err(ModelError::InvalidChange)
    );
    // Lane has never been read on creation; legacy text updates remain no-ops.
    let no_lane = changes(created.changes().iter().cloned().map(|mut change| {
        if let Change::Create {
            kind: ObjectKind::Clip,
            fields,
            ..
        } = &mut change
        {
            fields.retain(|field| field.property != Property::Lane);
        }
        change
    }));
    assert_eq!(replay(&empty, &no_lane), base);
    for id in [target, Id::from_raw(0)] {
        let changed = changes([update(
            id,
            Property::Lane,
            Value::Text("timeline".into()),
            Value::Text("legacy".into()),
        )]);
        assert_eq!(replay(&base, &changed), base);
    }
}

#[test]
fn placement_changesets_resolve_ownership_content_and_track_orders_separately() {
    let (base, target, other) = fixture();
    let (base, destination) = base.add_track("destination");
    let (owner, before) = clip(&base, target);
    let after = with_values(before, other, (17, 53, -3, true, 0x3eaa_aaab, 1, 2, true));
    let mut edits: Vec<_> = fields(destination, &after)
        .into_iter()
        .map(|field| update(target, field.property, field.value.clone(), field.value))
        .collect();
    edits.extend([
        update(
            owner,
            Property::ClipOrder,
            Value::Ids(vec![target]),
            Value::Ids(vec![]),
        ),
        update(
            destination,
            Property::ClipOrder,
            Value::Ids(vec![]),
            Value::Ids(vec![target]),
        ),
    ]);
    let right = base.apply_changeset(&changes(edits)).unwrap();
    assert_eq!(clip(&right, target), (destination, &after));
    check_roundtrip(&base, &right, target);

    for (property, value) in [
        (Property::Owner, Value::Id(destination)),
        (Property::Owner, Value::Id(Id::from_raw(0))),
        (Property::Content, Value::Id(Id::from_raw(0))),
    ] {
        let changed = changes([update(target, property, value.clone(), value)]);
        assert_eq!(
            base.apply_changeset(&changed),
            Err(ModelError::InvalidChange),
            "unresolved placement {property:?}"
        );
    }
    for order in [vec![], vec![target, target], vec![Id::from_raw(0)]] {
        let changed = changes([update(
            owner,
            Property::ClipOrder,
            Value::Ids(vec![target]),
            Value::Ids(order.clone()),
        )]);
        assert_eq!(
            base.apply_changeset(&changed),
            Err(ModelError::InvalidChange),
            "invalid placement order {order:?}"
        );
    }
    let deleted = changes([Change::Delete {
        kind: ObjectKind::Clip,
        id: ObjectKey::Id(target),
        fields: vec![],
    }]);
    assert_eq!(
        base.apply_changeset(&deleted),
        Err(ModelError::InvalidChange)
    );
    assert_eq!(
        clip(&base, target).1,
        before,
        "refused replay changed its source"
    );
}
