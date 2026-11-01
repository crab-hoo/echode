//! Note replay expectations independent of the changeset field table.
//!
//! Arbitrary stored values exercise the low-level API, not valid project notes.
//! Publication and package loading still enforce the model invariants.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, Note, NoteDraft, ObjectKey, ObjectKind, Project,
    Property, Tick, Value,
};
use proptest::prelude::*;

fn fixture() -> (Project, Id, Id) {
    let (project, track) = Project::new().add_track("note fields");
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(100),
            },
        )
        .unwrap();
    let content = project.tracks()[0].clips[0].content;
    (project, content, note)
}

fn note(project: &Project, content: Id) -> &Note {
    &project.content(content).unwrap().notes[0]
}

fn field(property: Property, value: Value) -> Field {
    Field { property, value }
}

fn fields(owner: Id, note: &Note) -> Vec<Field> {
    vec![
        field(Property::Owner, Value::Id(owner)),
        field(Property::Start, Value::Tick(note.start)),
        field(Property::Length, Value::Tick(note.length)),
        field(Property::Pitch, Value::Byte(note.pitch)),
        field(Property::Velocity, Value::U16(note.velocity)),
        field(Property::Release, Value::U16(note.release)),
        field(Property::Articulation, Value::Parent(note.articulation)),
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

fn write(project: &Project, owner: Id, value: &Note) -> Project {
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

fn check_roundtrip(base: &Project, owner: Id, before: &Note, after: &Note) {
    let left = write(base, owner, before);
    let right = write(base, owner, after);
    assert_eq!(note(&left, owner), before, "note writes crossed fields");
    assert_eq!(note(&right, owner), after, "note writes lost stored values");
    let diff = left.diff(&right);
    let expected: Vec<_> = fields(owner, before)
        .into_iter()
        .zip(fields(owner, after))
        .filter(|(left, right)| left.value != right.value)
        .map(|(left, right)| update(after.id, left.property, left.value, right.value))
        .collect();
    assert_eq!(
        diff.changes(),
        expected,
        "note diff changed its wire fields"
    );
    assert_eq!(replay(&left, &diff), right);
    assert_eq!(replay(&right, &diff.invert()), left);
    assert_eq!(diff.invert().invert(), diff);

    let empty = Project::new();
    let created = empty.diff(&right);
    let found = created
        .changes()
        .iter()
        .find_map(|change| match change {
            Change::Create {
                kind: ObjectKind::Note,
                id: ObjectKey::Id(id),
                fields,
            } if *id == after.id => Some(fields),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        *found,
        fields(owner, after),
        "note creation changed its wire fields"
    );
    assert_eq!(replay(&empty, &created), right);
    assert_eq!(replay(&right, &created.invert()), empty);
    let deleted = right.diff(&empty);
    let found = deleted
        .changes()
        .iter()
        .find_map(|change| match change {
            Change::Delete {
                kind: ObjectKind::Note,
                id: ObjectKey::Id(id),
                fields,
            } if *id == after.id => Some(fields),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        *found,
        fields(owner, after),
        "note deletion changed its wire fields"
    );
    assert_eq!(replay(&right, &deleted), empty);
    assert_eq!(replay(&empty, &deleted.invert()), right);
}

fn stored_note() -> impl Strategy<Value = Note> {
    (
        any::<i64>(),
        any::<i64>(),
        any::<u8>(),
        any::<u16>(),
        any::<u16>(),
        proptest::option::of(any::<u128>()),
    )
        .prop_map(
            |(start, length, pitch, velocity, release, articulation)| Note {
                id: Id::from_raw(0),
                start: Tick::new(start),
                length: Tick::new(length),
                pitch,
                velocity,
                release,
                articulation: articulation.map(Id::from_raw),
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn note_changesets_preserve_arbitrary_stored_values(mut before in stored_note(), mut after in stored_note()) {
        let (base, owner, id) = fixture();
        before.id = id;
        after.id = id;
        check_roundtrip(&base, owner, &before, &after);
        check_roundtrip(&base, owner, &after, &after);
    }
}

#[test]
fn note_changesets_keep_all_field_combinations_and_integer_boundaries() {
    let (base, owner, id) = fixture();
    let before = Note {
        id,
        start: Tick::new(i64::MIN),
        length: Tick::new(i64::MAX),
        pitch: u8::MAX,
        velocity: 1,
        release: u16::MAX,
        articulation: None,
    };
    let alternate = Note {
        id,
        start: Tick::new(i64::MAX),
        length: Tick::new(0),
        pitch: 0,
        velocity: u16::MAX,
        release: 0,
        articulation: Some(Id::from_raw(u128::MAX)),
    };
    for mask in 0..64 {
        let mut after = fields(owner, &before);
        for (index, field) in fields(owner, &alternate).into_iter().skip(1).enumerate() {
            if mask & (1 << index) != 0 {
                after[index + 1] = field;
            }
        }
        let project = base
            .apply_changeset(&changes(after.into_iter().map(|field| {
                update(id, field.property, field.value.clone(), field.value)
            })))
            .unwrap();
        check_roundtrip(&base, owner, &before, note(&project, owner));
    }
    // Articulation removal and negative stored lengths are low-level replay, too.
    check_roundtrip(
        &base,
        owner,
        &alternate,
        &Note {
            length: Tick::new(i64::MIN),
            ..before
        },
    );
}

#[test]
fn note_changesets_reject_missing_fields_wrong_variants_and_unknown_targets() {
    let (base, owner, id) = fixture();
    let empty = Project::new();
    let created = empty.diff(&base);
    for field in fields(owner, note(&base, owner)) {
        for wrong_type in [false, true] {
            let changed = changes(created.changes().iter().cloned().map(|mut change| {
                if let Change::Create {
                    kind: ObjectKind::Note,
                    fields,
                    ..
                } = &mut change
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
                "note creation {:?}, wrong_type={wrong_type}",
                field.property
            );
        }
        for (target, value) in [
            (id, Value::Ids(vec![])),
            (Id::from_raw(0), field.value.clone()),
        ] {
            let changed = changes([update(target, field.property, field.value.clone(), value)]);
            assert_eq!(
                base.apply_changeset(&changed),
                Err(ModelError::InvalidChange),
                "note update {:?}, target={target}",
                field.property
            );
        }
    }
    let changed = changes([update(
        id,
        Property::Key,
        Value::Text("old".into()),
        Value::Text("new".into()),
    )]);
    assert_eq!(
        base.apply_changeset(&changed),
        Err(ModelError::InvalidChange)
    );
    // Legacy lane writes are ignored; they must not become a writable note field.
    let changed = changes([update(
        id,
        Property::Lane,
        Value::Text("timeline".into()),
        Value::Text("timeline".into()),
    )]);
    assert_eq!(replay(&base, &changed), base);
    let changed = changes([update(
        id,
        Property::Owner,
        Value::Id(owner),
        Value::Id(Id::from_raw(0)),
    )]);
    assert_eq!(
        base.apply_changeset(&changed),
        Err(ModelError::InvalidChange)
    );
    assert_eq!(
        note(&base, owner).id,
        id,
        "refused replay changed its source"
    );
}

#[test]
fn note_changesets_move_ownership_only_with_matching_content_orders() {
    let (base, owner, id) = fixture();
    let (base, track) = base.add_track("destination");
    let (base, other) = base
        .insert_note(
            track,
            NoteDraft {
                pitch: 72,
                velocity: 90,
                start: Tick::new(1000),
                length: Tick::new(100),
            },
        )
        .unwrap();
    let destination = base.tracks()[1].clips[0].content;
    let after = base
        .apply_changeset(&changes([
            update(
                id,
                Property::Owner,
                Value::Id(owner),
                Value::Id(destination),
            ),
            update(
                owner,
                Property::NoteOrder,
                Value::Ids(vec![id]),
                Value::Ids(vec![]),
            ),
            update(
                destination,
                Property::NoteOrder,
                Value::Ids(vec![other]),
                Value::Ids(vec![id, other]),
            ),
            update(
                id,
                Property::Start,
                Value::Tick(Tick::new(0)),
                Value::Tick(Tick::new(1)),
            ),
        ]))
        .unwrap();
    assert!(after.content(owner).unwrap().notes.is_empty());
    assert_eq!(
        note(&after, destination),
        &Note {
            start: Tick::new(1),
            ..note(&base, owner).clone()
        }
    );
    let diff = base.diff(&after);
    let note_updates: Vec<_> = diff
        .changes()
        .iter()
        .filter(|change| {
            matches!(change, Change::Update { id: ObjectKey::Id(found), .. } if *found == id)
        })
        .cloned()
        .collect();
    assert_eq!(
        note_updates,
        vec![
            update(
                id,
                Property::Owner,
                Value::Id(owner),
                Value::Id(destination)
            ),
            update(
                id,
                Property::Start,
                Value::Tick(Tick::new(0)),
                Value::Tick(Tick::new(1))
            ),
        ],
        "note ownership must precede scalar updates"
    );
    assert_eq!(replay(&base, &diff), after);
    assert_eq!(replay(&after, &diff.invert()), base);
    let unmatched = changes([update(
        id,
        Property::Owner,
        Value::Id(owner),
        Value::Id(destination),
    )]);
    assert_eq!(
        base.apply_changeset(&unmatched),
        Err(ModelError::InvalidChange)
    );
}
