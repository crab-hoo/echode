use super::*;

#[test]
fn asynchronous_preparation_failure_is_reported_once_and_releases_pending_plans() {
    let (base, track) = Project::new().add_track("error fixture");
    // This block size bounds only this failure/lifecycle fixture.
    let (mut control, mut audio) = playback(&base, 48_000, 64, PlaybackOptions::default()).unwrap();
    control.prepare_in_background();
    // The workgroup provider rejects before any helper is spawned. The existing
    // executable is only an existent route path, never loaded as a plugin.
    let path = std::env::current_exe().unwrap();
    control.set_routes(
        &path,
        &[PluginRoute {
            clap_id: "org.example.failure".into(),
            path: path.clone(),
            instrument: false,
            fault: None,
            state: None,
        }],
    );
    control.workgroup = Some(Arc::new(|| Err("fixture workgroup failure".into())));
    let changed = base
        .add_clap_device(track, echode_model::Id::generate(), "org.example.failure")
        .unwrap();
    control.replace(&changed).unwrap();
    assert!(control.preparing_graph() && control.status().pending_plan);
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    let error = loop {
        control.service();
        if let Some(error) = control.take_preparation_error() {
            break error;
        }
        assert!(
            Instant::now() < deadline,
            "background failure was not reported"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert!(error.to_string().contains("fixture workgroup failure"));
    assert!(!control.preparing_graph() && !control.status().pending_plan);
    for _ in 0..4 {
        control.service();
        assert!(control.take_preparation_error().is_none());
    }
    // A later valid replacement can still be prepared and adopted.
    control.replace(&base).unwrap();
    let mut left = [0.0; 64];
    let mut right = left;
    while control.status().pending_plan {
        control.service();
        audio.render(&mut left, &mut right);
        assert!(control.take_preparation_error().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
