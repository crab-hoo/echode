use super::*;

#[test]
fn recording_the_same_bytes_twice_adds_one_undo_step() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::from_raw(0xD0);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    assert!(record_state(&host, device, b"edited").unwrap());
    assert!(!record_state(&host, device, b"edited").unwrap());
    let hash = echode_history::object_hash(b"edited");
    let state = |host: &DocumentHost| host.snapshot().tracks()[0].devices[0].state.clone();
    assert_eq!(state(&host), Some(hash.clone()));
    host.undo().unwrap();
    assert_eq!(state(&host), None);
    host.redo().unwrap();
    assert_eq!(state(&host), Some(hash));
    assert!(matches!(
        record_state(&host, Id::from_raw(0xD1), b"gone"),
        Err(DocumentError::Rejected(Rejected::DeviceGone))
    ));
    drop(host);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn device_commands_refuse_unknown_fields_and_the_mixer() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, _) = echode_model::Project::new().add_track("fx");
    let mixer = project.tracks()[0].devices[0].id;
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    let mut editing = BTreeSet::new();
    let unknown = br#"{"type":"device.parameters","device":"00000000-0000-0000-0000-0000000000d0","extra":1}"#;
    assert!(matches!(
        dispatch(&host, &mut audio, &mut editing, unknown, false),
        Err(DocumentError::Malformed(_))
    ));
    let mixer_request = format!(r#"{{"type":"device.parameters","device":"{mixer}"}}"#);
    let Err(error) = dispatch(
        &host,
        &mut audio,
        &mut editing,
        mixer_request.as_bytes(),
        false,
    ) else {
        panic!("the mixer has no helper");
    };
    assert!(
        matches!(error, DocumentError::Rejected(Rejected::Mixer)),
        "{error}"
    );
    let empty = format!(r#"{{"type":"device.parameter.set","device":"{mixer}","changes":[]}}"#);
    assert!(matches!(
        dispatch(&host, &mut audio, &mut editing, empty.as_bytes(), false),
        Err(DocumentError::Rejected(Rejected::Mixer))
    ));
    let parse =
        format!(r#"{{"type":"device.parameter.parse","device":"{mixer}","id":1,"text":"1"}}"#);
    let Err(error) = dispatch(&host, &mut audio, &mut editing, parse.as_bytes(), false) else {
        panic!("the mixer has no plugin text");
    };
    assert!(
        matches!(error, DocumentError::Rejected(Rejected::Mixer)),
        "{error}"
    );
    let untyped = format!(r#"{{"type":"device.parameter.parse","device":"{mixer}","id":1}}"#);
    assert!(matches!(
        dispatch(&host, &mut audio, &mut editing, untyped.as_bytes(), false),
        Err(DocumentError::Malformed(_))
    ));
    let window = format!(r#"{{"type":"device.gui.show","device":"{mixer}","title":"Mix"}}"#);
    let Err(error) = dispatch(&host, &mut audio, &mut editing, window.as_bytes(), false) else {
        panic!("the mixer has no plugin window");
    };
    assert!(
        matches!(error, DocumentError::Rejected(Rejected::Mixer)),
        "{error}"
    );
    assert!(editing.is_empty());
    drop(host);
    let _ = std::fs::remove_dir_all(dir);
}

/// Empty changes are refused as such on a plugin device, before the
/// output opens or the device counts as edited.
#[test]
fn empty_parameter_changes_are_refused_before_the_output_opens() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::from_raw(0xD3);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    let mut editing = BTreeSet::new();
    let empty = format!(r#"{{"type":"device.parameter.set","device":"{device}","changes":[]}}"#);
    let refused = dispatch(&host, &mut audio, &mut editing, empty.as_bytes(), false);
    assert!(
        matches!(
            refused,
            Err(DocumentError::Rejected(Rejected::ParameterChanges))
        ),
        "{:?}",
        refused.err()
    );
    assert!(editing.is_empty() && !audio.opening());
    drop(host);
    let _ = std::fs::remove_dir_all(dir);
}

/// Closing a window never opens the output or starts a helper to do it:
/// this unresolvable plugin would fail if the close tried.
#[test]
fn closing_a_window_without_a_helper_does_nothing() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::from_raw(0xD2);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    let mut editing = BTreeSet::new();
    let close = format!(r#"{{"type":"device.gui.close","device":"{device}"}}"#);
    let Resolution::Ready(dispatched) =
        dispatch(&host, &mut audio, &mut editing, close.as_bytes(), false).unwrap()
    else {
        panic!("closing never waits for an output");
    };
    assert!(dispatched.control.is_none());
    let report = round_trip(dispatched.control.as_ref(), &dispatched.request).unwrap();
    assert_eq!(report, br#"{"closed":true}"#);
    drop(host);
    let _ = std::fs::remove_dir_all(dir);
}

/// DIA-02: a failed commit must end the edit, leaving later background
/// saves eligible for recording without another commit from the app.
#[test]
fn audit_dia02_a_failed_commit_does_not_hold_later_background_saves() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::from_raw(0xD4);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    let mut saves = crate::saves::PluginSaves::default();
    saves.editing.insert(device.raw());
    // A save already waiting for this edit must also be released on failure.
    saves.add(vec![(device.raw(), b"edited".to_vec())]);
    // The store cannot be written, so recording the captured bytes fails.
    std::fs::write(dir.join("media"), b"not a directory").unwrap();
    let failed = commit(
        &host,
        &mut audio,
        &mut saves.editing,
        device,
        Ok(b"edited".to_vec()),
    );
    std::fs::remove_file(dir.join("media")).unwrap();
    assert!(matches!(failed, Err(DocumentError::Package(_))));
    assert_eq!(host.history_usage().unwrap().undo_steps, 0);
    // The plugin then saves on its own, as after any change. The newest wins.
    saves.add(vec![(device.raw(), b"later".to_vec())]);
    let recorded = saves.record(|id, bytes| {
        assert_eq!(id, device.raw());
        assert_eq!(bytes, b"later");
        host.commit_plugin_state(Id::from_raw(id), bytes).unwrap()
    });
    assert!(
        recorded,
        "the save waits for a commit the app never sends again"
    );
    assert_eq!(
        host.history_usage().unwrap().undo_steps,
        0,
        "a background save adds no Undo step"
    );
    drop(host);
    let reopened = DocumentHost::open(&dir).unwrap();
    assert_eq!(
        reopened.snapshot().tracks()[0].devices[0].state,
        Some(echode_history::object_hash(b"later"))
    );
    drop(reopened);
    let _ = std::fs::remove_dir_all(dir);
}

/// Both a changed and an unchanged successful commit end only this device's
/// edit. Equal bytes still add no Undo step.
#[test]
fn successful_commits_release_only_their_own_edit() {
    let dir = std::env::temp_dir().join(format!("echode-document-device-{}", Id::generate()));
    let (project, track) = echode_model::Project::new().add_track("fx");
    let device = Id::from_raw(0xD4);
    let other = Id::from_raw(0xD5);
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    echode_history::write_package(&dir, &project).unwrap();
    let host = DocumentHost::open(&dir).unwrap();
    let mut audio = Session::new(host.snapshot());
    for changed in [true, false] {
        let mut editing = BTreeSet::from([device.raw(), other.raw()]);
        let report = commit(
            &host,
            &mut audio,
            &mut editing,
            device,
            Ok(b"edited".to_vec()),
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&report).unwrap();
        assert_eq!(report["changed"], changed);
        assert_eq!(editing, BTreeSet::from([other.raw()]));
        assert_eq!(host.history_usage().unwrap().undo_steps, 1);
    }
    host.undo().unwrap();
    assert_eq!(host.snapshot().tracks()[0].devices[0].state, None);
    host.redo().unwrap();
    assert_eq!(
        host.snapshot().tracks()[0].devices[0].state,
        Some(echode_history::object_hash(b"edited"))
    );
    drop(host);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_changes_a_commit_records_defer_background_saves() {
    use echode_audio_io::{ParamEvent, ParamEventKind};
    let mut activity = PluginActivity::default();
    activity.relay.rescan = true;
    activity.closed = true;
    assert!(
        !defers_saves(&activity),
        "a rescan or a closed window records nothing"
    );
    activity.closed = false;
    activity.window_state = Some(true);
    assert!(
        !defers_saves(&activity),
        "a window lifetime is not a plugin edit"
    );
    let value = ParamEvent {
        id: 1,
        kind: ParamEventKind::Value,
    };
    let mut closed_window = PluginActivity::default();
    closed_window.relay.events.push(value);
    closed_window.relay.lost = true;
    assert!(
        !defers_saves(&closed_window),
        "without a window, a plugin's own value stays a background save"
    );
    for change in [
        |activity: &mut PluginActivity| {
            activity.relay.events.push(ParamEvent {
                id: 1,
                kind: ParamEventKind::Begin,
            })
        },
        |activity: &mut PluginActivity| {
            activity.window_open = true;
            activity.relay.events.push(ParamEvent {
                id: 1,
                kind: ParamEventKind::Value,
            });
        },
        |activity: &mut PluginActivity| {
            activity.window_open = true;
            activity.relay.lost = true;
        },
        |activity: &mut PluginActivity| activity.dirty = true,
    ] {
        let mut edited = PluginActivity::default();
        change(&mut edited);
        assert!(defers_saves(&edited));
    }
}
