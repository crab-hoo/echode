use echode_model::{Id, Project};

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
