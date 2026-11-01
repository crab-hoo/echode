//! Content replay expectations independent of the changeset field table.
//!
//! Arbitrary take ids exercise stored values, not a take-editing workflow.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    AudioRef, Change, ChangeSet, Content, Field, Id, ModelError, NoteDraft, ObjectKey, ObjectKind,
    Project, Property, Tick, Value,
};
use proptest::prelude::*;

fn fixture() -> (Project, Id, Id) {
    let mut project = Project::new();
    let mut contents = Vec::new();
    for name in ["content fields", "other content"] {
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
        contents.push(project.tracks().last().unwrap().clips[0].content);
    }
    let source = project.tracks()[0].clips[0].id;
    let (project, linked) = project.add_track("linked content");
    let project = project
        .link_clip(source, linked, Tick::new(100), Id::generate())
        .unwrap();
    (project, contents[0], contents[1])
}

fn field(property: Property, value: Value) -> Field {
    Field { property, value }
}

fn fields(content: &Content) -> Vec<Field> {
    vec![
        field(Property::Audio, Value::Audio(content.audio.clone())),
        field(Property::Takes, Value::Ids(content.takes.clone())),
        field(
            Property::NoteOrder,
            Value::Ids(content.notes.iter().map(|note| note.id).collect()),
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

fn write(project: &Project, content: &Content) -> Project {
    project
        .apply_changeset(&changes(fields(content).into_iter().map(|field| {
            update(content.id, field.property, field.value.clone(), field.value)
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
    let diff = left.diff(right);
    let expected: Vec<_> = fields(left.content(target).unwrap())
        .into_iter()
        .zip(fields(right.content(target).unwrap()))
        .filter(|(left, right)| left.value != right.value)
        .map(|(left, right)| update(target, left.property, left.value, right.value))
        .collect();
    assert_eq!(
        diff.changes(),
        expected,
        "content diff changed its wire fields"
    );
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
                    kind: ObjectKind::Content,
                    id: ObjectKey::Id(id),
                    fields,
                } if create && *id == target => Some(fields),
                Change::Delete {
                    kind: ObjectKind::Content,
                    id: ObjectKey::Id(id),
                    fields,
                } if !create && *id == target => Some(fields),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            *actual,
            fields(right.content(target).unwrap()),
            "content creation/deletion changed its wire fields"
        );
        assert_eq!(replay(source, &diff), *destination);
        assert_eq!(replay(destination, &diff.invert()), *source);
    }
}

fn audio() -> impl Strategy<Value = Option<AudioRef>> {
    proptest::option::of(
        (
            any::<u128>(),
            any::<u128>(),
            1u32..=u32::MAX,
            1u16..=2,
            1u64..=u64::MAX,
        )
            .prop_map(|(first, second, sample_rate, channels, frames)| {
                AudioRef::try_new(
                    format!("{first:032x}{second:032x}"),
                    sample_rate,
                    channels,
                    frames,
                )
                .unwrap()
            }),
    )
}

fn takes() -> impl Strategy<Value = Vec<Id>> {
    // This vector length is a regression budget, not a product limit.
    prop::collection::vec(any::<u128>().prop_map(Id::from_raw), 0..8)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn content_changesets_preserve_audio_metadata_and_take_order(
        before_audio in audio(), after_audio in audio(),
        before_takes in takes(), after_takes in takes(),
    ) {
        let (base, target, _) = fixture();
        let before = Content { audio: before_audio, takes: before_takes, ..base.content(target).unwrap().clone() };
        let after = Content { audio: after_audio, takes: after_takes, ..before.clone() };
        let left = write(&base, &before);
        let right = write(&base, &after);
        assert_eq!(left.content(target).unwrap(), &before, "content writes crossed fields");
        assert_eq!(right.content(target).unwrap(), &after, "content writes lost stored values");
        check_roundtrip(&left, &right, target);
    }
}

#[test]
fn content_changesets_cover_all_field_combinations() {
    let (base, target, other) = fixture();
    // This mask enumerates the two stored fields, not a product limit.
    for mask in 0..4 {
        let after = Content {
            audio: (mask & 1 != 0)
                .then(|| AudioRef::try_new("ab".repeat(32), 96_000, 2, u64::MAX).unwrap()),
            takes: if mask & 2 != 0 {
                vec![other, Id::from_raw(0), other]
            } else {
                vec![]
            },
            ..base.content(target).unwrap().clone()
        };
        let right = write(&base, &after);
        check_roundtrip(&base, &right, target);
        assert_eq!(base.diff(&right).is_empty(), mask == 0);
    }
}

#[test]
fn content_changesets_keep_note_order_before_fields_and_replay_legacy_lists() {
    let (base, target, other) = fixture();
    let (right, added) = base
        .insert_note(
            base.tracks()[0].id,
            NoteDraft {
                pitch: 64,
                velocity: 90,
                start: Tick::new(100),
                length: Tick::new(100),
            },
        )
        .unwrap();
    let after = Content {
        audio: Some(AudioRef::try_new("cd".repeat(32), 44_100, 1, 100).unwrap()),
        takes: vec![other],
        ..right.content(target).unwrap().clone()
    };
    let right = write(&right, &after);
    let diff = base.diff(&right);
    let updates: Vec<_> = diff
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
        .collect();
    assert_eq!(
        updates,
        [Property::NoteOrder, Property::Audio, Property::Takes]
    );
    assert!(diff.changes().iter().any(|change| matches!(change,
        Change::Update { id: ObjectKey::Id(id), property: Property::NoteOrder,
            after: Value::OrderStep { index: 1, id: note, present: true }, .. }
            if *id == target && *note == added)));
    assert_eq!(replay(&base, &diff), right);
    assert_eq!(replay(&right, &diff.invert()), base);
    let legacy = changes(diff.changes().iter().cloned().map(|change| match change {
        Change::Update {
            id: ObjectKey::Id(id),
            property: Property::NoteOrder,
            ..
        } if id == target => update(
            target,
            Property::NoteOrder,
            fields(base.content(target).unwrap())[2].value.clone(),
            fields(&after)[2].value.clone(),
        ),
        other => other,
    }));
    assert_eq!(replay(&base, &legacy), right);
    assert_eq!(replay(&right, &legacy.invert()), base);
}

#[test]
fn content_changesets_keep_legacy_null_audio_and_lane_noop() {
    let (base, target, _) = fixture();
    let empty = Project::new();
    let legacy = changes(
        empty
            .diff(&base)
            .changes()
            .iter()
            .cloned()
            .map(|mut change| {
                if let Change::Create {
                    kind: ObjectKind::Content,
                    fields,
                    ..
                } = &mut change
                {
                    fields
                        .iter_mut()
                        .find(|field| field.property == Property::Audio)
                        .unwrap()
                        .value = Value::Parent(None);
                }
                change
            }),
    );
    assert_eq!(replay(&empty, &legacy), base);
    assert_eq!(replay(&base, &legacy.invert()), empty);
    let audio = Some(AudioRef::try_new("ef".repeat(32), 48_000, 2, 100).unwrap());
    let right = write(
        &base,
        &Content {
            audio: audio.clone(),
            ..base.content(target).unwrap().clone()
        },
    );
    let clear = changes([update(
        target,
        Property::Audio,
        Value::Audio(audio),
        Value::Parent(None),
    )]);
    assert_eq!(replay(&right, &clear), base);
    assert_eq!(replay(&base, &clear.invert()), right);
    let lane = changes([update(
        target,
        Property::Lane,
        Value::Text("timeline".into()),
        Value::Text("timeline".into()),
    )]);
    assert_eq!(replay(&base, &lane), base);
}

#[test]
fn content_changesets_reject_missing_fields_wrong_types_and_unknown_targets() {
    let (base, target, _) = fixture();
    let empty = Project::new();
    for field in fields(base.content(target).unwrap()) {
        for wrong_type in [false, true] {
            let invalid = changes(
                empty
                    .diff(&base)
                    .changes()
                    .iter()
                    .cloned()
                    .map(|mut change| {
                        if let Change::Create {
                            kind: ObjectKind::Content,
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
                    }),
            );
            assert_eq!(
                empty.apply_changeset(&invalid),
                Err(ModelError::InvalidChange),
                "content creation {:?}, wrong_type={wrong_type}",
                field.property
            );
        }
        for (id, value) in [
            (target, Value::Tick(Tick::new(0))),
            (Id::from_raw(0), field.value.clone()),
        ] {
            let invalid = changes([update(id, field.property, field.value.clone(), value)]);
            assert_eq!(
                base.apply_changeset(&invalid),
                Err(ModelError::InvalidChange),
                "content update {:?}, target={id}",
                field.property
            );
        }
    }
    for (property, value) in [
        (Property::Audio, Value::Parent(Some(target))),
        (Property::Key, Value::Text("key".into())),
    ] {
        let invalid = changes([update(target, property, value.clone(), value)]);
        assert_eq!(
            base.apply_changeset(&invalid),
            Err(ModelError::InvalidChange)
        );
    }
    let duplicate = changes([Change::Create {
        kind: ObjectKind::Content,
        id: ObjectKey::Id(target),
        fields: fields(base.content(target).unwrap()),
    }]);
    assert_eq!(
        base.apply_changeset(&duplicate),
        Err(ModelError::InvalidChange)
    );
}

#[test]
fn content_changesets_keep_child_resolution_and_order_step_checks() {
    let (base, target, other) = fixture();
    let note = base.content(target).unwrap().notes[0].id;
    let foreign = base.content(other).unwrap().notes[0].id;
    for after in [
        Value::Ids(vec![]),
        Value::Ids(vec![Id::from_raw(0)]),
        Value::Ids(vec![foreign]),
        Value::Ids(vec![note, note]),
        Value::OrderStep {
            index: 2,
            id: note,
            present: true,
        },
        Value::OrderStep {
            index: 1,
            id: note,
            present: false,
        },
        Value::OrderStep {
            index: 0,
            id: foreign,
            present: false,
        },
    ] {
        let invalid = changes([update(
            target,
            Property::NoteOrder,
            Value::Ids(vec![note]),
            after.clone(),
        )]);
        assert_eq!(
            base.apply_changeset(&invalid),
            Err(ModelError::InvalidChange),
            "unresolved order {after:?}"
        );
    }
    let wrong_owner = changes([update(
        note,
        Property::Owner,
        Value::Id(target),
        Value::Id(other),
    )]);
    assert_eq!(
        base.apply_changeset(&wrong_owner),
        Err(ModelError::InvalidChange)
    );
    assert_eq!(
        base.check_invariants(),
        Ok(()),
        "refused replay changed its source"
    );
}
