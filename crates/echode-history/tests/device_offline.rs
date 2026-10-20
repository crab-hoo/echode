use echode_history::{read_package, write_package};
use echode_model::{Id, Project};

#[test]
fn schema_four_migrates_online_and_schema_five_requires_valid_offline_flags() {
    let directory = std::env::temp_dir().join(format!("echode-offline-schema-{}", Id::generate()));
    let (project, track) = Project::new().add_track("legacy");
    let id = Id::generate();
    let project = project
        .add_clap_device(track, id, "org.example.fx")
        .unwrap()
        .set_device_bypassed(id, true)
        .unwrap();
    write_package(&directory, &project).unwrap();
    let track_path = directory.join("tracks").join(format!("{track}.json"));
    for path in [directory.join("project.json"), track_path.clone()] {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["schema"] = 4.into();
        if let Some(devices) = value
            .get_mut("devices")
            .and_then(serde_json::Value::as_array_mut)
        {
            for device in devices {
                device.as_object_mut().unwrap().remove("offline");
            }
        }
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    assert_eq!(read_package(&directory).unwrap(), project);
    let offline = project.set_device_offline(id, true).unwrap();
    write_package(&directory, &offline).unwrap();
    assert_eq!(read_package(&directory).unwrap(), offline);
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&track_path).unwrap()).unwrap();
    let mut missing = original.clone();
    missing["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("offline");
    std::fs::write(&track_path, serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(read_package(&directory).is_err());
    for malformed in [serde_json::Value::Null, "true".into(), 1.into()] {
        let mut value = original.clone();
        value["devices"][0]["offline"] = malformed;
        std::fs::write(&track_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read_package(&directory).is_err());
    }
    let mut mixer = original;
    mixer["devices"][1]["offline"] = true.into();
    std::fs::write(&track_path, serde_json::to_vec(&mixer).unwrap()).unwrap();
    assert!(read_package(&directory).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}
