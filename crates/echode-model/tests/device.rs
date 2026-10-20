use echode_model::{Change, ChangeSet, Id, ObjectKey, ObjectKind, Project, Property, Value};

#[test]
fn offline_preserves_state_and_bypass_through_copy_and_changesets() {
    let (project, track) = Project::new().add_track("offline");
    let id = Id::generate();
    let project = project
        .add_clap_device(track, id, "org.example.fx")
        .unwrap()
        .set_device_state(id, Some("a".repeat(64)))
        .unwrap()
        .set_device_bypassed(id, true)
        .unwrap();
    let offline = project.set_device_offline(id, true).unwrap();
    assert!(offline.tracks()[0].devices[0].offline);
    assert!(offline.tracks()[0].devices[0].bypassed);
    assert_eq!(offline.tracks()[0].devices[0].state, Some("a".repeat(64)));
    let diff = project.diff(&offline);
    assert_eq!(project.apply_changeset(&diff).unwrap(), offline);
    assert_eq!(offline.apply_changeset(&diff.invert()).unwrap(), project);
    let copied = offline
        .copy_device(id, track, Id::generate(), None)
        .unwrap();
    assert!(
        copied.tracks()[0].devices[..2]
            .iter()
            .all(|device| device.offline && device.bypassed)
    );
    copied.check_invariants().unwrap();
    assert_eq!(offline.set_device_offline(id, false).unwrap(), project);
    assert!(project.set_device_offline(Id::generate(), true).is_err());
    assert!(
        project
            .set_device_offline(project.tracks()[0].devices.last().unwrap().id, true)
            .is_err()
    );
    let mut legacy = ChangeSet::new();
    for change in project.diff(&copied).changes() {
        let mut change = change.clone();
        if let Change::Create {
            kind: ObjectKind::Device,
            fields,
            ..
        } = &mut change
        {
            fields.retain(|field| field.property != Property::Offline);
        }
        legacy.push(change);
    }
    let restored = project.apply_changeset(&legacy).unwrap();
    assert!(!restored.tracks()[0].devices[1].offline);
}

#[test]
fn legacy_device_creation_without_bypass_replays_as_active() {
    let (before, track) = Project::new().add_track("legacy");
    let after = before
        .add_clap_device(track, Id::generate(), "org.example.effect")
        .unwrap();
    let mut legacy = ChangeSet::new();
    for change in before.diff(&after).changes() {
        let mut change = change.clone();
        if let Change::Create {
            kind: ObjectKind::Device,
            fields,
            ..
        } = &mut change
        {
            fields.retain(|field| field.property != Property::Bypassed);
        }
        legacy.push(change);
    }
    assert_eq!(before.apply_changeset(&legacy).unwrap(), after);
    assert_eq!(after.apply_changeset(&legacy.invert()).unwrap(), before);
}

#[test]
fn bypass_preserves_device_state_and_survives_copy_and_diff_inversion() {
    let (project, track) = Project::new().add_track("track");
    let device = Id::from_raw(100);
    let project = project
        .add_clap_device(track, device, "org.example.effect")
        .unwrap()
        .set_device_state(device, Some("a".repeat(64)))
        .unwrap();
    let bypassed = project.set_device_bypassed(device, true).unwrap();
    let value = &bypassed.tracks()[0].devices[0];
    assert!(value.bypassed);
    assert_eq!(value.id, device);
    assert_eq!(value.state, project.tracks()[0].devices[0].state);
    let diff = project.diff(&bypassed);
    assert_eq!(project.apply_changeset(&diff).unwrap(), bypassed);
    assert_eq!(bypassed.apply_changeset(&diff.invert()).unwrap(), project);
    let copied = bypassed
        .copy_device(device, track, Id::from_raw(101), None)
        .unwrap();
    assert!(copied.tracks()[0].devices[1].bypassed);
    copied.check_invariants().unwrap();
    assert_eq!(
        bypassed.set_device_bypassed(device, false).unwrap(),
        project
    );
    assert!(
        project
            .set_device_bypassed(project.tracks()[0].devices.last().unwrap().id, true)
            .is_err()
    );
    assert!(
        project
            .set_device_bypassed(Id::from_raw(999), false)
            .is_err()
    );
}

#[test]
fn a_journal_change_cannot_bypass_a_mixer_or_give_it_plugin_state() {
    let (project, _) = Project::new().add_track("track");
    let mixer = project.tracks()[0].devices.last().unwrap().id;
    for (property, after) in [
        (Property::Bypassed, Value::Byte(1)),
        (Property::Offline, Value::Byte(1)),
        (Property::PluginState, Value::Text("a".repeat(64))),
        (Property::PinnedParameters, Value::ParamIds(vec![1])),
    ] {
        let mut changes = ChangeSet::new();
        changes.push(Change::Update {
            id: ObjectKey::Id(mixer),
            property,
            before: match property {
                Property::Bypassed | Property::Offline => Value::Byte(0),
                Property::PinnedParameters => Value::ParamIds(Vec::new()),
                _ => Value::Text(String::new()),
            },
            after,
        });
        assert!(project.apply_changeset(&changes).is_err(), "{property:?}");
    }
}

#[test]
fn chain_reorder_and_copy_keep_identity_settings_and_mixer() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let a = Id::from_raw(100);
    let b = Id::from_raw(101);
    let copy = Id::from_raw(102);
    let project = project
        .add_clap_device(first, a, "org.example.a")
        .unwrap()
        .add_clap_device(first, b, "org.example.b")
        .unwrap()
        .set_device_state(a, Some("a".repeat(64)))
        .unwrap();
    let mixer = project.tracks()[0].devices.last().unwrap().clone();
    let moved = project.reorder_device(b, Some(a)).unwrap();
    assert_eq!(
        moved.tracks()[0]
            .devices
            .iter()
            .map(|d| d.id)
            .collect::<Vec<_>>(),
        vec![b, a, mixer.id]
    );
    assert_eq!(moved.tracks()[0].devices.last(), Some(&mixer));
    let duplicated = moved.copy_device(a, second, copy, None).unwrap();
    assert_eq!(duplicated.tracks()[0].devices, moved.tracks()[0].devices);
    let created = &duplicated.tracks()[1].devices[0];
    assert_eq!(created.id, copy);
    assert_eq!(created.plugin, moved.tracks()[0].devices[1].plugin);
    assert_eq!(created.state, Some("a".repeat(64)));
    duplicated.check_invariants().unwrap();
    for (before, after) in [(&project, &moved), (&moved, &duplicated)] {
        let changes = before.diff(after);
        assert_eq!(before.apply_changeset(&changes).unwrap(), *after);
        assert_eq!(after.apply_changeset(&changes.invert()).unwrap(), *before);
    }
    let ended = moved.reorder_device(b, None).unwrap();
    assert_eq!(
        ended.tracks()[0]
            .devices
            .iter()
            .map(|d| d.id)
            .collect::<Vec<_>>(),
        vec![a, b, mixer.id]
    );
    assert!(
        moved
            .diff(&moved.reorder_device(b, Some(b)).unwrap())
            .is_empty()
    );
    assert!(
        moved
            .diff(&moved.reorder_device(a, None).unwrap())
            .is_empty()
    );
    assert!(moved.reorder_device(mixer.id, Some(a)).is_err());
    let other_mixer = moved.tracks()[1].devices[0].id;
    assert!(moved.reorder_device(a, Some(other_mixer)).is_err());
    assert!(moved.copy_device(mixer.id, second, copy, None).is_err());
    assert!(moved.copy_device(a, second, b, None).is_err());
    assert!(moved.copy_device(a, second, copy, Some(b)).is_err());
    assert!(moved.copy_device(a, Id::from_raw(999), copy, None).is_err());
    assert_eq!(project.tracks()[0].devices[0].id, a);
}

#[test]
fn a_plugin_is_inserted_before_the_mixer_and_the_mixer_stays() {
    let (project, track) = Project::new().add_track("piano");
    let device = Id::from_raw(40);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-gain")
        .unwrap();
    let again = project
        .add_clap_device(track, Id::from_raw(41), "dev.echode.test-gain")
        .unwrap();
    let devices = &again.tracks()[0].devices;
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0].plugin.as_deref(), Some("dev.echode.test-gain"));
    assert_eq!(devices[1].id, Id::from_raw(41));
    assert!(devices[2].plugin.is_none());
    assert!(devices[2].parameters.len() == 4);
    let removed = again.remove_device(device).unwrap();
    assert_eq!(removed.tracks()[0].devices.len(), 2);
    assert!(
        removed
            .remove_device(removed.tracks()[0].devices[1].id)
            .is_err()
    );
    assert!(project.add_clap_device(track, device, "").is_err());
    assert!(project.add_clap_device(track, device, "bad\nid").is_err());
    let changes = project.diff(&again);
    assert_eq!(project.apply_changeset(&changes).unwrap(), again);
    assert_eq!(again.apply_changeset(&changes.invert()).unwrap(), project);
}

#[test]
fn pins_stay_sorted_and_survive_copy_duplicate_and_changesets() {
    let (project, track) = Project::new().add_track("pins");
    let id = Id::generate();
    let project = project
        .add_clap_device(track, id, "org.example.fx")
        .unwrap();
    let pinned = project
        .set_parameter_pinned(id, 9, true)
        .unwrap()
        .set_parameter_pinned(id, 3, true)
        .unwrap();
    assert_eq!(pinned.tracks()[0].devices[0].pinned, [3, 9]);
    // Pinning again or unpinning an absent id changes nothing.
    assert_eq!(pinned.set_parameter_pinned(id, 3, true).unwrap(), pinned);
    assert_eq!(pinned.set_parameter_pinned(id, 4, false).unwrap(), pinned);
    let diff = project.diff(&pinned);
    assert_eq!(project.apply_changeset(&diff).unwrap(), pinned);
    assert_eq!(pinned.apply_changeset(&diff.invert()).unwrap(), project);
    let released = pinned.set_parameter_pinned(id, 9, false).unwrap();
    assert_eq!(released.tracks()[0].devices[0].pinned, [3]);
    let step = pinned.diff(&released);
    assert_eq!(released.apply_changeset(&step.invert()).unwrap(), pinned);

    // A copy is the same settings under a new identity.
    let copied = pinned.copy_device(id, track, Id::generate(), None).unwrap();
    assert!(
        copied.tracks()[0].devices[..2]
            .iter()
            .all(|device| device.pinned == [3, 9])
    );
    let duplicated = pinned.duplicate_track(track, Id::generate()).unwrap();
    assert_eq!(duplicated.tracks()[1].devices[0].pinned, [3, 9]);
    duplicated.check_invariants().unwrap();
    // Each copy keeps its own list.
    let copy = copied.tracks()[0].devices[1].id;
    let edited = copied.set_parameter_pinned(copy, 3, false).unwrap();
    assert_eq!(edited.tracks()[0].devices[0].pinned, [3, 9]);
    assert_eq!(edited.tracks()[0].devices[1].pinned, [9]);

    let mixer = project.tracks()[0].devices.last().unwrap().id;
    assert!(project.set_parameter_pinned(mixer, 1, true).is_err());
    assert!(project.set_parameter_pinned(id, u32::MAX, true).is_err());
    assert!(
        project
            .set_parameter_pinned(Id::generate(), 1, true)
            .is_err()
    );
    for invalid in [vec![9, 3], vec![3, 3], vec![u32::MAX]] {
        let mut changes = ChangeSet::new();
        changes.push(Change::Update {
            id: ObjectKey::Id(id),
            property: Property::PinnedParameters,
            before: Value::ParamIds(Vec::new()),
            after: Value::ParamIds(invalid.clone()),
        });
        assert!(project.apply_changeset(&changes).is_err(), "{invalid:?}");
    }

    // A device created by a journal from before pins existed has none.
    let mut legacy = ChangeSet::new();
    for change in project.diff(&copied).changes() {
        let mut change = change.clone();
        if let Change::Create {
            kind: ObjectKind::Device,
            fields,
            ..
        } = &mut change
        {
            fields.retain(|field| field.property != Property::PinnedParameters);
        }
        legacy.push(change);
    }
    let restored = project.apply_changeset(&legacy).unwrap();
    assert!(restored.tracks()[0].devices[1].pinned.is_empty());
}
