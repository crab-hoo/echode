use super::*;
use echode_sandbox::{PluginHost, SessionLayout, SpawnOptions};

#[test]
fn contract_audio_config_selection_records_plugin_state_once_and_releases_failures() {
    check_edit(
        "device.audio_config.select",
        r#", "id":206"#,
        r#", "id":1"#,
        None,
    );
}

#[test]
fn contract_audio_port_changes_record_plugin_state_once_and_release_failures() {
    check_edit(
        "device.audio_ports.apply",
        r#", "requests":[{"input":false,"index":0,"channels":1}]"#,
        r#", "requests":[{"input":false,"index":4294967295,"channels":1}]"#,
        None,
    );
}
#[test]
fn contract_audio_port_details_cross_the_device_lane_and_state_history() {
    check_edit(
        "device.audio_ports.apply",
        r#", "requests":[{"input":false,"index":0,"channels":1,"details":{"type":"surround","channel_map":[2]}}]"#,
        r#", "requests":[{"input":false,"index":0,"channels":1,"details":{"type":"surround","channel_map":[0,1]}}]"#,
        Some(1),
    );
    check_edit(
        "device.audio_ports.apply",
        r#", "requests":[{"input":false,"index":0,"channels":1,"details":{"type":"ambisonic","ordering":1,"normalization":1}}]"#,
        r#", "requests":[{"input":false,"index":0,"channels":1,"details":{"type":"ambisonic","ordering":4294967295,"normalization":1}}]"#,
        Some(2),
    );
}

fn check_edit(kind: &str, extra: &str, refused_extra: &str, details_marker: Option<u8>) {
    let artifacts = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut plugin = PluginHost::spawn(SpawnOptions {
        host_bin: &artifacts.join("echode-plugin-host"),
        plugin: Some(&artifacts.join("libechode_test_plugin.dylib")),
        fault: Some("audio-ports"),
        plugin_id: Some("dev.echode.test-gain"),
        layout: SessionLayout::stereo(),
        sample_rate: 48_000,
        workgroup: None,
        fan_in: None,
        cancel: None,
    })
    .expect("build the product helper and test plugin before this regression");
    let control = plugin.control_handle();
    let list: serde_json::Value =
        serde_json::from_slice(&round_trip(Some(&control), &Request::AudioConfigs).unwrap())
            .unwrap();
    assert_eq!(list["audio_configs"]["current_id"], 100);
    assert_eq!(list["audio_configs"]["configs"][1]["id"], 206);
    assert_eq!(
        list["audio_configs"]["configs"][1]["main_output_channels"],
        1
    );

    let dir = std::env::temp_dir().join(format!("echode-config-state-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::generate();
    let initial = control.capture_state().unwrap();
    let hash = echode_history::write_object(&dir, &initial).unwrap();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-gain")
        .unwrap()
        .set_device_state(device, Some(hash.clone()))
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    let mut editing = BTreeSet::new();
    let bytes = format!(r#"{{"type":"{kind}","device":"{device}"{extra}}}"#);
    let Resolution::Waiting(waiting) =
        dispatch(&host, &mut audio, &mut editing, bytes.as_bytes(), true)
            .expect("typed audio-port details must reach the device lane")
    else {
        panic!("configuration edits must reach the device lane");
    };
    let dispatched = waiting.ready(Some(control.clone()), &mut editing);
    assert!(
        editing.contains(&device.raw()),
        "defer background saves before changing ports"
    );
    let captured = round_trip(dispatched.control.as_ref(), &dispatched.request);
    assert_eq!(
        captured.as_ref().unwrap()[3],
        6,
        "change ports before capturing plugin-owned layout"
    );
    let selected = captured.as_ref().unwrap().clone();
    assert_eq!(
        captured.as_ref().unwrap().get(4).copied(),
        details_marker,
        "preserve typed details in plugin-owned state"
    );
    let report = commit(&host, &mut audio, &mut editing, device, captured).unwrap();
    assert_eq!(report, br#"{"changed":true}"#);
    assert!(editing.is_empty());
    let changed_hash = echode_history::object_hash(&selected);
    assert_eq!(
        host.snapshot().tracks()[0].devices[0].state.as_deref(),
        Some(changed_hash.as_str())
    );
    assert_eq!(
        echode_history::read_object(&dir, &changed_hash).unwrap(),
        selected
    );
    let repeated = Ok(selected.clone());
    assert_eq!(
        commit(&host, &mut audio, &mut editing, device, repeated).unwrap(),
        br#"{"changed":false}"#
    );
    host.undo().unwrap();
    assert_eq!(
        host.snapshot().tracks()[0].devices[0].state.as_deref(),
        Some(hash.as_str())
    );
    control.restore_state(&initial).unwrap();
    assert_eq!(control.capture_state().unwrap().get(4).copied(), None);
    let list: serde_json::Value =
        serde_json::from_slice(&round_trip(Some(&control), &Request::AudioConfigs).unwrap())
            .unwrap();
    assert_eq!(list["audio_configs"]["current_id"], 100);
    host.redo().unwrap();
    control.restore_state(&selected).unwrap();
    assert_eq!(
        control.capture_state().unwrap().get(4).copied(),
        details_marker,
        "restore typed details from the saved state"
    );
    let list: serde_json::Value =
        serde_json::from_slice(&round_trip(Some(&control), &Request::AudioConfigs).unwrap())
            .unwrap();
    assert_eq!(list["audio_configs"]["current_id"], 206);
    assert!(plugin.follow_ports().unwrap());

    editing.insert(device.raw());
    let bytes = format!(r#"{{"type":"{kind}","device":"{device}"{refused_extra}}}"#);
    let Resolution::Waiting(waiting) =
        dispatch(&host, &mut audio, &mut editing, bytes.as_bytes(), true).unwrap()
    else {
        panic!("refused edits must reach the device lane");
    };
    let dispatched = waiting.ready(Some(control.clone()), &mut editing);
    let refused = round_trip(dispatched.control.as_ref(), &dispatched.request);
    assert!(matches!(refused, Err(DocumentError::Helper(_))));
    assert!(commit(&host, &mut audio, &mut editing, device, refused).is_err());
    assert!(
        editing.is_empty(),
        "failed configuration edits must release background saves"
    );
    assert_eq!(
        host.snapshot().tracks()[0].devices[0].state.as_deref(),
        Some(changed_hash.as_str())
    );
    drop(host);
    let reopened = DocumentHost::open(&dir).unwrap();
    assert_eq!(
        reopened.snapshot().tracks()[0].devices[0].state.as_deref(),
        Some(changed_hash.as_str())
    );
    drop(reopened);
    plugin.shutdown().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
