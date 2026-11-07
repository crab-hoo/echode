//! Device counts bound these changeset fixtures only, not product chains.

use super::{fixture, measured};
use crate::{Change, ChangeSet, Device, Id, ObjectKey, ObjectKind, Project, Property, Value};
use std::sync::Arc;

fn chain(count: usize) -> Project {
    let project = fixture(4);
    let (project, _) = project.add_track("unrelated");
    let mut tracks: Vec<_> = project
        .tracks()
        .iter()
        .map(|track| (**track).clone())
        .collect();
    let mixer = tracks[0].devices.pop().unwrap();
    tracks[0].devices = (0..count)
        .map(|index| {
            // Repeated plugin IDs must not be mistaken for device identity.
            let mut device =
                Device::clap(Id::from_raw(1_000 + index as u128), "org.example.fx".into());
            device.state = Some("a".repeat(64));
            device.pinned = vec![1, 9];
            device
        })
        .chain(std::iter::once(mixer))
        .collect();
    Project::try_assemble(
        project.meter(),
        project.tempo().clone(),
        project
            .contents()
            .iter()
            .map(|content| (**content).clone())
            .collect(),
        tracks,
    )
    .unwrap()
}

fn diff(before: &Project, after: &Project) -> ChangeSet {
    let (changes, work) = measured(|| before.diff(after));
    let devices = |project: &Project| {
        project
            .tracks()
            .iter()
            .map(|track| track.devices.len())
            .sum::<usize>()
    };
    let limit = 3 * devices(before).max(devices(after));
    assert!(
        work.diff_device_keys <= limit,
        "device diff inspected {} keys for chains with at most {} devices (limit {limit})",
        work.diff_device_keys,
        devices(before).max(devices(after))
    );
    assert_eq!(work.diff_note_visits, 0, "device diff walked shared notes");
    changes
}

fn round_trip(before: &Project, after: &Project, changes: &ChangeSet) {
    before.check_invariants().unwrap();
    after.check_invariants().unwrap();
    assert_eq!(before.apply_changeset(changes).unwrap(), *after);
    assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), *before);
}

#[test]
fn device_diffs_do_not_rescan_the_chain_for_each_device() {
    for count in [64, 256] {
        let before = chain(count);
        let track = &before.tracks()[0];
        let mixer = track.devices.last().unwrap();
        for index in [0, count / 2, count - 1] {
            let device = track.devices[index].id;
            let after = before.set_device_bypassed(device, true).unwrap();
            let changes = diff(&before, &after);
            assert_eq!(
                changes.changes(),
                &[Change::Update {
                    id: ObjectKey::Id(device),
                    property: Property::Bypassed,
                    before: Value::Byte(0),
                    after: Value::Byte(1),
                }]
            );
            assert!(!track.devices[index].bypassed);
            assert!(Arc::ptr_eq(&before.tracks()[1], &after.tracks()[1]));
            round_trip(&before, &after, &changes);
        }
        // A mixer edit also compares all preceding CLAP devices.
        let gain = mixer.parameters[0].id;
        let after = before.set_parameter(gain, 0.5).unwrap();
        let changes = diff(&before, &after);
        assert_eq!(
            changes.changes(),
            &[Change::Update {
                id: ObjectKey::Id(gain),
                property: Property::Value,
                before: Value::Bits(1.0f32.to_bits()),
                after: Value::Bits(0.5f32.to_bits()),
            }]
        );
        round_trip(&before, &after, &changes);
    }
}

#[test]
fn device_diffs_keep_scalar_updates_in_chain_order() {
    for count in [64, 256] {
        let before = chain(count);
        let track = &before.tracks()[0];
        let first = track.devices[0].id;
        let last = track.devices[count - 1].id;
        let after = before
            .set_device_state(first, None)
            .unwrap()
            .set_device_offline(last, true)
            .unwrap()
            .set_parameter_pinned(last, 9, false)
            .unwrap();
        let changes = diff(&before, &after);
        assert_eq!(
            changes.changes(),
            &[
                Change::Update {
                    id: ObjectKey::Id(first),
                    property: Property::PluginState,
                    before: Value::Text("a".repeat(64)),
                    after: Value::Text(String::new()),
                },
                Change::Update {
                    id: ObjectKey::Id(last),
                    property: Property::PinnedParameters,
                    before: Value::ParamIds(vec![1, 9]),
                    after: Value::ParamIds(vec![1]),
                },
                Change::Update {
                    id: ObjectKey::Id(last),
                    property: Property::Offline,
                    before: Value::Byte(0),
                    after: Value::Byte(1),
                },
            ]
        );
        round_trip(&before, &after, &changes);
    }
}

#[test]
fn device_diffs_keep_reordering_creation_and_deletion_order() {
    for count in [64, 256] {
        let before = chain(count);
        let track = &before.tracks()[0];
        let first = track.devices[0].id;
        let last = track.devices[count - 1].id;
        let reordered = before.reorder_device(last, Some(first)).unwrap();
        let changes = diff(&before, &reordered);
        assert_eq!(
            changes.changes(),
            &[Change::Update {
                id: ObjectKey::Id(track.id),
                property: Property::DeviceOrder,
                before: Value::Ids(track.devices.iter().map(|device| device.id).collect()),
                after: Value::Ids(
                    reordered.tracks()[0]
                        .devices
                        .iter()
                        .map(|device| device.id)
                        .collect()
                ),
            }]
        );
        round_trip(&before, &reordered, &changes);

        let copy = Id::from_raw(100_000);
        let after = reordered
            .copy_device(last, track.id, copy, Some(first))
            .unwrap()
            .remove_device(first)
            .unwrap();
        let changes = diff(&reordered, &after);
        assert_eq!(changes.changes().len(), 3);
        assert!(matches!(changes.changes()[0], Change::Create {
            kind: ObjectKind::Device, id: ObjectKey::Id(id), ..
        } if id == copy));
        assert!(matches!(
            changes.changes()[1],
            Change::Update {
                property: Property::DeviceOrder,
                ..
            }
        ));
        assert!(matches!(changes.changes()[2], Change::Delete {
            kind: ObjectKind::Device, id: ObjectKey::Id(id), ..
        } if id == first));
        assert_eq!(after.tracks()[0].devices[1].state, track.devices[0].state);
        round_trip(&reordered, &after, &changes);

        let other_track = before.tracks()[1].id;
        let copied = before.copy_device(last, other_track, copy, None).unwrap();
        let changes = diff(&before, &copied);
        assert!(Arc::ptr_eq(&before.tracks()[0], &copied.tracks()[0]));
        assert_eq!(
            copied.tracks()[1].devices[0].state,
            track.devices[count - 1].state
        );
        round_trip(&before, &copied, &changes);
    }
}

#[test]
fn device_diffs_handle_shared_equal_and_disjoint_chains() {
    assert!(diff(&Project::new(), &Project::new()).is_empty());
    for count in [0, 1, 64, 256] {
        let before = chain(count);
        let (changes, work) = measured(|| before.diff(&before.clone()));
        assert!(changes.is_empty());
        assert_eq!(work.diff_device_keys, 0, "shared tracks were not skipped");
        let contents = before
            .contents()
            .iter()
            .map(|content| (**content).clone())
            .collect();
        let mut tracks: Vec<_> = before
            .tracks()
            .iter()
            .map(|track| (**track).clone())
            .collect();
        let equal = Project::try_assemble(
            before.meter(),
            before.tempo().clone(),
            contents,
            tracks.clone(),
        )
        .unwrap();
        assert!(!Arc::ptr_eq(&before.tracks()[0], &equal.tracks()[0]));
        let (changes, work) = measured(|| before.diff(&equal));
        let device_count = count + 2;
        assert!(
            work.diff_device_keys <= 3 * device_count,
            "equal device chains inspected {} keys (limit {})",
            work.diff_device_keys,
            3 * device_count
        );
        assert!(changes.is_empty());

        for device in &mut tracks[0].devices {
            device.id = Id::generate();
            for parameter in &mut device.parameters {
                parameter.id = Id::generate();
            }
        }
        // Keep contents shared: this measures devices, not equal-content note diffs.
        let after = before.with_parts(
            before.contents().to_vec(),
            tracks.into_iter().map(Arc::new).collect(),
        );
        let changes = diff(&before, &after);
        let identities: Vec<_> = changes
            .changes()
            .iter()
            .map(|change| match change {
                Change::Create { kind, id, .. } => (true, *kind, *id),
                Change::Delete { kind, id, .. } => (false, *kind, *id),
                Change::Update { id, property, .. } => {
                    assert_eq!(*property, Property::DeviceOrder);
                    (true, ObjectKind::Track, *id)
                }
            })
            .collect();
        let mut expected = Vec::new();
        for device in &after.tracks()[0].devices {
            expected.push((true, ObjectKind::Device, ObjectKey::Id(device.id)));
            expected.extend(
                device
                    .parameters
                    .iter()
                    .map(|parameter| (true, ObjectKind::Parameter, ObjectKey::Id(parameter.id))),
            );
        }
        expected.push((
            true,
            ObjectKind::Track,
            ObjectKey::Id(before.tracks()[0].id),
        ));
        for device in &before.tracks()[0].devices {
            expected.extend(
                device
                    .parameters
                    .iter()
                    .rev()
                    .map(|parameter| (false, ObjectKind::Parameter, ObjectKey::Id(parameter.id))),
            );
            expected.push((false, ObjectKind::Device, ObjectKey::Id(device.id)));
        }
        assert_eq!(identities, expected);
        round_trip(&before, &after, &changes);
    }
}
