use echode_history::{read_package, write_package};
use echode_model::{Id, Project};

#[test]
fn schema_five_migrates_without_pins_and_schema_six_requires_valid_pins() {
    let directory = std::env::temp_dir().join(format!("echode-pins-schema-{}", Id::generate()));
    let (project, track) = Project::new().add_track("legacy");
    let id = Id::generate();
    let project = project
        .add_clap_device(track, id, "org.example.fx")
        .unwrap();
    write_package(&directory, &project).unwrap();
    let track_path = directory.join("tracks").join(format!("{track}.json"));
    for path in [directory.join("project.json"), track_path.clone()] {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["schema"] = 5.into();
        if let Some(devices) = value
            .get_mut("devices")
            .and_then(serde_json::Value::as_array_mut)
        {
            for device in devices {
                device.as_object_mut().unwrap().remove("pinned");
            }
        }
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    assert_eq!(read_package(&directory).unwrap(), project);

    let pinned = project
        .set_parameter_pinned(id, 7, true)
        .unwrap()
        .set_parameter_pinned(id, 2, true)
        .unwrap();
    assert_eq!(pinned.tracks()[0].devices[0].pinned, [2, 7]);
    write_package(&directory, &pinned).unwrap();
    assert_eq!(read_package(&directory).unwrap(), pinned);
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&track_path).unwrap()).unwrap();
    assert_eq!(original["devices"][0]["pinned"], serde_json::json!([2, 7]));
    let mut missing = original.clone();
    missing["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("pinned");
    std::fs::write(&track_path, serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(read_package(&directory).is_err());
    for malformed in [
        serde_json::Value::Null,
        serde_json::json!([7, 2]),
        serde_json::json!([2, 2]),
        serde_json::json!([u32::MAX]),
        serde_json::json!([-1]),
        serde_json::json!(["2"]),
    ] {
        let mut value = original.clone();
        value["devices"][0]["pinned"] = malformed.clone();
        std::fs::write(&track_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(read_package(&directory).is_err(), "{malformed}");
    }
    let mut mixer = original;
    mixer["devices"][1]["pinned"] = serde_json::json!([1]);
    std::fs::write(&track_path, serde_json::to_vec(&mixer).unwrap()).unwrap();
    assert!(read_package(&directory).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}
