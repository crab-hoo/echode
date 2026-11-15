use super::*;

#[test]
fn session_publishes_the_compensated_playhead_without_moving_the_processing_clock() {
    let (project, track) = Project::new().add_track("delayed");
    let project = project
        .add_clap_device(track, echode_model::Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let mut session = Session::new(Arc::new(project.clone()));
    session.use_headless_output(crate::HeadlessOutput::default());
    session.perform(AudioCommand::Prepare).unwrap();
    until_opened(&mut session);
    // The normal product build supplies these fixtures; never open HAL or GUI.
    let host = echode_plugin_host::host_binary().expect("build the plugin helper first");
    let plugin = host.parent().unwrap().join("libechode_test_plugin.dylib");
    assert!(plugin.is_file(), "build echode-test-plugin first");
    session.stream.as_mut().unwrap().set_test_plugin_routes(
        &host,
        &[echode_engine::PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: plugin,
            instrument: false,
            fault: Some("delay:37".into()),
            state: None,
        }],
    );
    session
        .stream
        .as_mut()
        .unwrap()
        .replace_test_plan(&project)
        .unwrap();
    until_published(&mut session);
    assert_eq!(
        session.stream.as_ref().unwrap().status().latency_samples,
        37
    );
    let start = echode_model::TICKS_PER_QUARTER;
    session
        .perform(AudioCommand::SetStart { tick: start })
        .unwrap();
    assert_eq!(session.state().tick, start);
    assert_eq!(session.position().0, start);
    session.perform(AudioCommand::Play).unwrap();
    let origin =
        echode_engine::tick_to_sample(echode_model::Tick::new(start), project.tempo(), 48_000)
            .unwrap();
    let tick_at = |sample| {
        echode_engine::sample_to_tick(sample, project.tempo(), 48_000)
            .unwrap()
            .get()
    };
    // Wait for the seek's asynchronous plugin cleanup, not a fixed block count.
    until_running(&mut session);
    let status = session.stream.as_ref().unwrap().status();
    assert!(status.sample >= origin + 37);
    assert_eq!(status.tick, tick_at(status.sample));
    let audible = status.sample - 37;
    let held = tick_at(audible);
    assert_eq!(
        session.state().tick,
        held,
        "session exposed the processing clock"
    );
    assert_eq!(
        session.position().0,
        held,
        "document exposed the processing clock"
    );
    session.perform(AudioCommand::Pause).unwrap();
    // This count bounds only the regression, not callback geometry.
    session.stream.as_ref().unwrap().pump(17);
    assert_eq!(session.state().tick, held);
    assert_eq!(session.position().0, held);
    session.perform(AudioCommand::Play).unwrap();
    session.stream.as_ref().unwrap().pump(17);
    assert_eq!(session.state().tick, tick_at(audible + 17));
    session.perform(AudioCommand::Stop).unwrap();
    assert_eq!(session.state().tick, start);
    assert_eq!(session.position().0, start);
    session.stream.as_ref().unwrap().pump(17);
    assert_eq!(session.state().tick, start);
    assert_eq!(session.position().0, start);
    assert!(session.state().error.is_none());
}
