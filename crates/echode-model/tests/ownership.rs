//! Ownership rejections generated from `ObjectKind::owners`.
//!
//! Every owner list gets the same cases, so a new kind or a new owner of an
//! existing kind is checked without hand-written fixtures. The fixture must
//! hold every kind; the coverage assertion fails until a new kind is added to it.

use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, NoteDraft, ObjectKey, ObjectKind, Ownership, Project,
    Property, Tempo, Tick, Value,
};

fn note(quarter: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(quarter).unwrap(),
        length: Tick::quarters(1).unwrap(),
    }
}

/// Two owners of every list, each with at least one child.
fn fixture() -> Project {
    let mut project = Project::new().with_tempo(
        Tick::quarters(16).unwrap(),
        Tempo::quarters_per_minute(90).unwrap(),
    );
    for name in ["first", "second"] {
        let (next, track) = project.add_track(name);
        let (next, _) = next.insert_note(track, note(0)).unwrap();
        let (next, _) = next.insert_note(track, note(1)).unwrap();
        project = next
            .add_clap_device(track, Id::generate(), "org.example.fx")
            .unwrap();
    }
    project.check_invariants().unwrap();
    project
}

/// An object as the creation changeset from an empty project names it.
struct Object {
    kind: ObjectKind,
    id: Id,
    fields: Vec<Field>,
}

fn objects(project: &Project) -> Vec<Object> {
    let creation = Project::new().diff(project);
    for kind in ObjectKind::ALL {
        assert!(
            creation.changes().iter().any(
                |change| matches!(change, Change::Create { kind: found, .. } if found == kind)
            ),
            "the fixture has no {kind:?}; add one so its ownership is checked"
        );
    }
    creation
        .changes()
        .iter()
        .filter_map(|change| match change {
            Change::Create {
                kind,
                id: ObjectKey::Id(id),
                fields,
            } => Some(Object {
                kind: *kind,
                id: *id,
                fields: fields.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn list(object: &Object, order: Property) -> Vec<Id> {
    match object.fields.iter().find(|field| field.property == order) {
        Some(Field {
            value: Value::Ids(ids),
            ..
        }) => ids.clone(),
        _ => Vec::new(),
    }
}

fn changes(items: impl IntoIterator<Item = Change>) -> ChangeSet {
    let mut result = ChangeSet::new();
    for item in items {
        result.push(item);
    }
    result
}

fn relist(owner: &Object, order: Property, after: Vec<Id>) -> Change {
    Change::Update {
        id: ObjectKey::Id(owner.id),
        property: order,
        before: Value::Ids(list(owner, order)),
        after: Value::Ids(after),
    }
}

fn appended(owner: &Object, order: Property, id: Id) -> Change {
    let mut ids = list(owner, order);
    ids.push(id);
    relist(owner, order, ids)
}

/// A copy of `template` with a new identity and owner and no children of its own.
fn new_child(template: &Object, id: Id, owner: Id) -> Change {
    let mut fields = template.fields.clone();
    for field in &mut fields {
        let own_list = ObjectKind::ALL
            .iter()
            .flat_map(|kind| kind.owners())
            .any(|list| list.owner == template.kind && list.order == field.property);
        if field.property == Property::Owner {
            field.value = Value::Id(owner);
        } else if own_list {
            field.value = Value::Ids(Vec::new());
        }
    }
    Change::Create {
        kind: template.kind,
        id: ObjectKey::Id(id),
        fields,
    }
}

/// `kind` and every kind it owns, directly or through other kinds.
fn subtree(kind: ObjectKind) -> Vec<ObjectKind> {
    let mut kinds = vec![kind];
    let mut grew = true;
    while grew {
        grew = false;
        for candidate in ObjectKind::ALL {
            let owned = candidate
                .owners()
                .iter()
                .any(|list| kinds.contains(&list.owner));
            if owned && !kinds.contains(candidate) {
                kinds.push(*candidate);
                grew = true;
            }
        }
    }
    kinds
}

fn rejected_cases(
    project: &Project,
    kind: ObjectKind,
    owned: Ownership,
) -> Vec<(String, ChangeSet)> {
    let objects = objects(project);
    let owners: Vec<&Object> = objects
        .iter()
        .filter(|object| object.kind == owned.owner && !list(object, owned.order).is_empty())
        .collect();
    assert!(
        owners.len() >= 2,
        "the fixture needs two {:?} owners listing {kind:?} children by {:?}",
        owned.owner,
        owned.order
    );
    let (first, second) = (owners[0], owners[1]);
    let first_list = list(first, owned.order);
    let child = first_list[0];
    let other_child = list(second, owned.order)[0];
    let template = objects
        .iter()
        .find(|object| object.id == child)
        .expect("listed children exist");
    assert_eq!(template.kind, kind);
    let order = owned.order;
    let fresh = Id::generate();

    let mut cases = vec![
        (
            "unlisted new child".to_string(),
            changes([new_child(template, fresh, first.id)]),
        ),
        (
            "new child naming an unknown owner".to_string(),
            changes([
                new_child(template, fresh, Id::generate()),
                appended(first, order, fresh),
            ]),
        ),
        (
            "new child listed by another owner".to_string(),
            changes([
                new_child(template, fresh, first.id),
                appended(second, order, fresh),
            ]),
        ),
        (
            "owner omits an existing child".to_string(),
            changes([relist(first, order, first_list[1..].to_vec())]),
        ),
        (
            "owner lists a child twice".to_string(),
            changes([appended(first, order, child)]),
        ),
        (
            "another owner also lists the child".to_string(),
            changes([appended(second, order, child)]),
        ),
        (
            "owner lists another owner's child in place of its own".to_string(),
            changes([relist(first, order, {
                let mut ids = first_list.clone();
                ids[0] = other_child;
                ids
            })]),
        ),
        (
            "owner lists a missing child".to_string(),
            changes([appended(first, order, Id::generate())]),
        ),
    ];
    let possible_owners: Vec<ObjectKind> = kind.owners().iter().map(|list| list.owner).collect();
    for other in ObjectKind::ALL {
        let Some(sample) = objects.iter().find(|object| object.kind == *other) else {
            // Tempo points are addressed by tick and cannot be named as owners.
            continue;
        };
        if !possible_owners.contains(other) {
            cases.push((
                format!("new child naming a {other:?} as its owner"),
                changes([new_child(template, Id::generate(), sample.id)]),
            ));
        }
        if *other != kind {
            cases.push((
                format!("owner lists a {other:?}"),
                changes([appended(first, order, sample.id)]),
            ));
        }
    }
    let kept = subtree(kind);
    cases.push((
        "owners deleted with their children left".to_string(),
        changes(
            project
                .diff(&Project::new())
                .changes()
                .iter()
                .filter(
                    |change| !matches!(change, Change::Delete { kind, .. } if kept.contains(kind)),
                )
                .cloned(),
        ),
    ));
    cases
}

fn collision_cases(
    project: &Project,
    kind: ObjectKind,
    owned: Ownership,
) -> Vec<(String, ChangeSet)> {
    let objects = objects(project);
    let first = objects
        .iter()
        .find(|object| object.kind == owned.owner && !list(object, owned.order).is_empty())
        .unwrap();
    let template = objects
        .iter()
        .find(|object| object.id == list(first, owned.order)[0])
        .unwrap();
    objects
        .iter()
        .filter(|object| object.kind != kind)
        .map(|object| {
            (
                format!("new child reusing the id of a {:?}", object.kind),
                changes([
                    new_child(template, object.id, first.id),
                    appended(first, owned.order, object.id),
                ]),
            )
        })
        .collect()
}

#[test]
fn every_owner_list_rejects_unlisted_misowned_and_duplicated_children() {
    let project = fixture();
    let original = project.clone();
    assert_eq!(
        project.apply_changeset(&project.diff(&Project::new())),
        Ok(Project::new()),
        "deleting every object with its owner is the accepted counterpart"
    );
    let objects = objects(&project);
    let mut checked = 0;
    for kind in ObjectKind::ALL {
        for owned in kind.owners() {
            // The accepted counterpart: the same new child, listed by its owner.
            let owner = objects
                .iter()
                .find(|object| object.kind == owned.owner && !list(object, owned.order).is_empty())
                .unwrap();
            let template = objects
                .iter()
                .find(|object| object.id == list(owner, owned.order)[0])
                .unwrap();
            let fresh = Id::generate();
            let listed = project
                .apply_changeset(&changes([
                    new_child(template, fresh, owner.id),
                    appended(owner, owned.order, fresh),
                ]))
                .unwrap_or_else(|error| panic!("{kind:?} listed by {owned:?}: {error:?}"));
            assert_ne!(listed, project, "{kind:?} listed by {owned:?} was dropped");

            for (name, changes) in rejected_cases(&project, *kind, *owned) {
                assert_eq!(
                    project.apply_changeset(&changes),
                    Err(ModelError::InvalidChange),
                    "{kind:?} by {owned:?}: {name} was not rejected"
                );
                assert_eq!(project, original, "{name} mutated the source snapshot");
                checked += 1;
            }
            for (name, changes) in collision_cases(&project, *kind, *owned) {
                // A collision with a top-level object may be found from either side.
                assert!(
                    matches!(
                        project.apply_changeset(&changes),
                        Err(ModelError::InvalidChange | ModelError::DuplicateId)
                    ),
                    "{kind:?} by {owned:?}: {name} was not rejected"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn object_kinds_round_trip_their_journal_names() {
    for kind in ObjectKind::ALL {
        assert_eq!(ObjectKind::parse(kind.as_str()), Some(*kind));
    }
    assert_eq!(ObjectKind::parse("lane"), None);
}
