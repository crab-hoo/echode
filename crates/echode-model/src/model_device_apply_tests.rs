//! Fixture sizes bound these device-ownership regressions only, not product projects.

use super::measured;
use crate::{
    Change, ChangeSet, Device, Field, Id, ModelError, ObjectKey, ObjectKind, Project, Property,
    Value,
};
use std::sync::Arc;

fn fixture(effect_counts: &[usize]) -> Project {
    let mut project = Project::new();
    for _ in effect_counts {
        project = project.add_track("device ownership").0;
    }
    let tracks = project
        .tracks()
        .iter()
        .zip(effect_counts)
        .map(|(track, count)| {
            let mut track = (**track).clone();
            let effects =
                (0..*count).map(|_| Device::clap(Id::generate(), "org.example.fx".into()));
            track.devices.splice(0..0, effects);
            Arc::new(track)
        })
        .collect();
    let project = project.with_parts(Vec::new(), tracks);
    project.check_invariants().unwrap();
    project
}

fn apply(before: &Project, after: &Project, changes: &ChangeSet) {
    let (result, work) = measured(|| before.apply_changeset(changes));
    let device_count = after
        .tracks()
        .iter()
        .map(|track| track.devices.len())
        .sum::<usize>();
    assert!(
        work.apply_device_owner_visits <= device_count,
        "device ownership validation visited {} devices for {} tracks and {} devices",
        work.apply_device_owner_visits,
        after.tracks().len(),
        device_count
    );
    let result = result.unwrap();
    assert_eq!(result, *after);
    result.check_invariants().unwrap();
}

fn round_trip(before: &Project, after: &Project) {
    let changes = before.diff(after);
    apply(before, after, &changes);
    apply(after, before, &changes.invert());
}

#[test]
fn device_ownership_validation_does_not_rescan_all_devices_per_track() {
    let empty = Project::new();
    apply(&empty, &empty, &ChangeSet::new());
    for count in [16, 64, 256] {
        let counts: Vec<_> = (0..count).map(|index| index % 8).collect();
        let before = fixture(&counts);
        apply(&before, &before, &ChangeSet::new());
        let mixer = before.tracks()[count - 1].devices.last().unwrap();
        let after = before.set_parameter(mixer.parameters[0].id, 0.5).unwrap();
        round_trip(&before, &after);
        round_trip(&empty, &before);
    }
}

#[test]
fn device_ownership_validation_preserves_chain_and_track_changes() {
    for counts in [vec![0], vec![256], vec![0, 1, 7, 3, 0, 2]] {
        let before = fixture(&counts);
        let (added, track) = before.add_track("new track");
        let added = added
            .add_clap_device(track, Id::generate(), "org.example.new")
            .unwrap();
        round_trip(&before, &added);
        round_trip(&added, &Project::new());

        for track in before.tracks() {
            if track.devices.len() < 3 {
                continue;
            }
            let first = track.devices[0].id;
            let last_effect = track.devices[track.devices.len() - 2].id;
            let reordered = before.reorder_device(last_effect, Some(first)).unwrap();
            round_trip(&before, &reordered);
            let removed = reordered.remove_device(first).unwrap();
            round_trip(&reordered, &removed);
            let copied = removed
                .copy_device(last_effect, track.id, Id::generate(), None)
                .unwrap();
            round_trip(&removed, &copied);
            // One long-chain case is enough; the mixed fixture exercises each owner.
            if counts.len() == 1 {
                break;
            }
        }
    }
}

fn update_order(track: Id, before: Vec<Id>, after: Vec<Id>) -> Change {
    Change::Update {
        id: ObjectKey::Id(track),
        property: Property::DeviceOrder,
        before: Value::Ids(before),
        after: Value::Ids(after),
    }
}

fn extra_device(owner: Id) -> Change {
    Change::Create {
        kind: ObjectKind::Device,
        id: ObjectKey::Id(Id::generate()),
        fields: vec![
            Field {
                property: Property::Owner,
                value: Value::Id(owner),
            },
            Field {
                property: Property::DeviceKind,
                value: Value::Text("clap".into()),
            },
            Field {
                property: Property::Plugin,
                value: Value::Text("org.example.unlisted".into()),
            },
            Field {
                property: Property::ParamOrder,
                value: Value::Ids(Vec::new()),
            },
        ],
    }
}

#[test]
fn device_ownership_validation_rejects_unlisted_missing_and_wrong_owner_devices() {
    let before = fixture(&[2, 2, 0]);
    let first = &before.tracks()[0];
    let second = &before.tracks()[1];
    let order: Vec<_> = first.devices.iter().map(|device| device.id).collect();
    let mut missing = order.clone();
    missing[0] = Id::generate();
    let mut wrong_owner = order.clone();
    wrong_owner[0] = second.devices[0].id;
    let mut duplicate = order.clone();
    duplicate.push(order[0]);
    let cases = [
        ("unlisted device", extra_device(first.id)),
        ("orphan device", extra_device(Id::generate())),
        (
            "unlisted existing device",
            update_order(first.id, order.clone(), order[1..].to_vec()),
        ),
        (
            "unknown device",
            update_order(first.id, order.clone(), missing),
        ),
        (
            "wrong owner with equal counts",
            update_order(first.id, order.clone(), wrong_owner),
        ),
        (
            "extra device reference",
            update_order(first.id, order.clone(), duplicate),
        ),
        (
            "deleted listed device",
            Change::Delete {
                kind: ObjectKind::Device,
                id: ObjectKey::Id(order[0]),
                fields: Vec::new(),
            },
        ),
        (
            "deleted owner",
            Change::Delete {
                kind: ObjectKind::Track,
                id: ObjectKey::Id(first.id),
                fields: Vec::new(),
            },
        ),
    ];
    let original = before.clone();
    for (name, change) in cases {
        let mut changes = ChangeSet::new();
        changes.push(change);
        assert_eq!(
            before.apply_changeset(&changes),
            Err(ModelError::InvalidChange),
            "{name} was not rejected"
        );
        assert_eq!(before, original, "{name} mutated the source snapshot");
    }
}
