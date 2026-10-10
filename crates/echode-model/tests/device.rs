use echode_model::{Id, Project};

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
