//! Parameter ownership must be complete before a replayed snapshot is published.

use std::path::Path;

use echode_actions::{JournalLine, JournalOp, encode_line};
use echode_history::{DocumentHost, read_package, recover, write_package};
use echode_model::{
    Change, ChangeSet, Id, ModelError, ObjectKey, ObjectKind, Project, Property, Value,
};

fn fixture() -> Project {
    let (project, _) = Project::new().add_track("first");
    project.add_track("second").0
}

fn changes(items: impl IntoIterator<Item = Change>) -> ChangeSet {
    let mut result = ChangeSet::new();
    for item in items {
        result.push(item);
    }
    result
}

fn extra_parameter(project: &Project, owner: Id, id: Id) -> Change {
    let mut created = Project::new()
        .diff(project)
        .changes()
        .iter()
        .find(|change| {
            matches!(
                change,
                Change::Create {
                    kind: ObjectKind::Parameter,
                    ..
                }
            )
        })
        .unwrap()
        .clone();
    let Change::Create {
        id: key, fields, ..
    } = &mut created
    else {
        unreachable!();
    };
    *key = ObjectKey::Id(id);
    fields
        .iter_mut()
        .find(|field| field.property == Property::Owner)
        .unwrap()
        .value = Value::Id(owner);
    created
}

fn parameter_order(project: &Project, after: Vec<Id>) -> Change {
    let device = &project.tracks()[0].devices[0];
    Change::Update {
        id: ObjectKey::Id(device.id),
        property: Property::ParamOrder,
        before: Value::Ids(
            device
                .parameters
                .iter()
                .map(|parameter| parameter.id)
                .collect(),
        ),
        after: Value::Ids(after),
    }
}

fn invalid_cases(project: &Project) -> Vec<(&'static str, ChangeSet)> {
    let track = &project.tracks()[0];
    let mixer = &track.devices[0];
    let order: Vec<_> = mixer
        .parameters
        .iter()
        .map(|parameter| parameter.id)
        .collect();
    let mut duplicate = order.clone();
    duplicate[1] = order[0];
    let mut wrong_owner = order.clone();
    wrong_owner[0] = project.tracks()[1].devices[0].parameters[0].id;
    let mut missing = order.clone();
    missing[0] = Id::generate();
    let removed = Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        Vec::new(),
        vec![(*project.tracks()[1]).clone()],
    )
    .unwrap();
    let retained_parameters = changes(
        project
            .diff(&removed)
            .changes()
            .iter()
            .filter(|change| {
                !matches!(
                    change,
                    Change::Delete {
                        kind: ObjectKind::Parameter,
                        ..
                    }
                )
            })
            .cloned(),
    );
    let mut cases = vec![
        (
            "unlisted new parameter",
            changes([extra_parameter(project, mixer.id, Id::generate())]),
        ),
        (
            "unknown owner",
            changes([extra_parameter(project, Id::generate(), Id::generate())]),
        ),
        (
            "track is not a device owner",
            changes([extra_parameter(project, track.id, Id::generate())]),
        ),
        ("deleted owner", retained_parameters),
        (
            "unlisted existing parameter",
            changes([parameter_order(project, order[1..].to_vec())]),
        ),
        (
            "duplicate replacing an omitted parameter",
            changes([parameter_order(project, duplicate)]),
        ),
        (
            "wrong owner",
            changes([parameter_order(project, wrong_owner)]),
        ),
        (
            "missing parameter",
            changes([parameter_order(project, missing)]),
        ),
    ];
    for (name, id) in [
        ("track identity collision", track.id),
        ("device identity collision", mixer.id),
    ] {
        let mut replaced = order.clone();
        replaced[0] = id;
        cases.push((
            name,
            changes([
                extra_parameter(project, mixer.id, id),
                parameter_order(project, replaced),
                Change::Delete {
                    kind: ObjectKind::Parameter,
                    id: ObjectKey::Id(order[0]),
                    fields: Vec::new(),
                },
            ]),
        ));
    }
    cases
}

#[test]
fn changeset_rejects_unconsumed_duplicate_and_misowned_parameters() {
    let project = fixture();
    let original = project.clone();
    for (name, changes) in invalid_cases(&project) {
        let result = project.apply_changeset(&changes);
        assert_eq!(result, Err(ModelError::InvalidChange), "accepted {name}");
        assert_eq!(project, original, "{name} mutated the original snapshot");
        project.check_invariants().unwrap();
    }
}

#[test]
fn parameter_ownership_is_checked_only_after_the_complete_transaction() {
    let project = fixture();
    let owner = project.tracks()[0].devices[0].id;
    let id = Id::generate();
    let created = extra_parameter(&project, owner, id);
    let Change::Create { fields, .. } = &created else {
        unreachable!();
    };
    let deleted = Change::Delete {
        kind: ObjectKind::Parameter,
        id: ObjectKey::Id(id),
        fields: fields.clone(),
    };
    let temporary = changes([created, deleted]);
    assert_eq!(project.apply_changeset(&temporary).unwrap(), project);
    assert_eq!(
        project.apply_changeset(&temporary.invert()).unwrap(),
        project
    );
    let empty = Project::new();
    let created = empty.diff(&project);
    assert_eq!(empty.apply_changeset(&created).unwrap(), project);
    assert_eq!(project.apply_changeset(&created.invert()).unwrap(), empty);
    let mixer = &project.tracks()[0].devices[0];
    let edited = project.set_parameter(mixer.parameters[0].id, 0.25).unwrap();
    let edit = project.diff(&edited);
    assert_eq!(project.apply_changeset(&edit).unwrap(), edited);
    assert_eq!(edited.apply_changeset(&edit.invert()).unwrap(), project);
}

fn tip(directory: &Path) -> Option<gix::ObjectId> {
    let repo = gix::open(directory).ok()?;
    Some(
        repo.try_find_reference("refs/echode/autosave/main")
            .ok()??
            .id()
            .detach(),
    )
}

fn assert_preserved(
    directory: &Path,
    project: &Project,
    journal: &[u8],
    original_tip: Option<gix::ObjectId>,
) {
    assert_eq!(read_package(directory).unwrap(), *project);
    assert_eq!(
        std::fs::read(directory.join("journal.jsonl")).unwrap(),
        journal
    );
    assert_eq!(tip(directory), original_tip);
    assert!(!directory.join("save.pending").exists());
}

#[test]
fn recovery_refuses_to_silently_discard_unlisted_parameters() {
    for with_tip in [false, true] {
        for op in [JournalOp::Edit, JournalOp::Undo, JournalOp::Redo] {
            let directory = std::env::temp_dir()
                .join(format!("echode-parameter-ownership-{}", Id::generate()))
                .join("song.echode");
            let project = fixture();
            write_package(&directory, &project).unwrap();
            if with_tip {
                drop(DocumentHost::open(&directory).unwrap());
            }
            let original_tip = tip(&directory);
            assert_eq!(original_tip.is_some(), with_tip);
            let gain = project.tracks()[0].devices[0].parameters[0].id;
            let edited = project.set_parameter(gain, 0.25).unwrap();
            let edit = project.diff(&edited);
            let corrupt = changes([extra_parameter(
                &edited,
                project.tracks()[0].devices[0].id,
                Id::generate(),
            )]);
            let records = [
                JournalLine {
                    seq: 1,
                    op: JournalOp::Edit,
                    actions: Vec::new(),
                    changes: edit,
                },
                JournalLine {
                    seq: 2,
                    op,
                    actions: Vec::new(),
                    changes: if op == JournalOp::Undo {
                        corrupt.invert()
                    } else {
                        corrupt
                    },
                },
            ];
            let journal = records
                .iter()
                .map(|line| format!("{}\n", encode_line(line)))
                .collect::<String>()
                .into_bytes();
            std::fs::write(directory.join("journal.jsonl"), &journal).unwrap();
            let error =
                recover(&directory).expect_err("recovery silently discarded unlisted parameters");
            assert!(
                error
                    .to_string()
                    .contains("journal seq 2 cannot apply changes:"),
                "{error}"
            );
            assert_preserved(&directory, &project, &journal, original_tip);
            // A refused product open must release the package lock for the next attempt.
            for _ in 0..2 {
                let error = DocumentHost::open(&directory)
                    .err()
                    .expect("product open silently discarded unlisted parameters");
                assert!(
                    error
                        .to_string()
                        .contains("journal seq 2 cannot apply changes:"),
                    "{error}"
                );
                assert_preserved(&directory, &project, &journal, original_tip);
            }
            // Only an explicit repair of the test fixture makes this document writable.
            std::fs::write(
                directory.join("journal.jsonl"),
                format!("{}\n", encode_line(&records[0])),
            )
            .unwrap();
            let host = DocumentHost::open(&directory).unwrap();
            assert_eq!(*host.snapshot(), edited);
            drop(host);
            let host = DocumentHost::open(&directory).unwrap();
            assert_eq!(*host.snapshot(), edited);
            drop(host);
            std::fs::remove_dir_all(directory.parent().unwrap()).unwrap();
        }
    }
}
