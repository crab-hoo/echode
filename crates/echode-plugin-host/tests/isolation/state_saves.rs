use super::*;

/// DIA-04: the first save takes three seconds, beyond the helper's two-second
/// wait. Its only dirty notice must survive the rejection and a later capture
/// must reach the parent without a second change in the plugin.
#[test]
fn a_background_capture_recovers_after_a_helper_timeout_without_another_dirty_notice() {
    let capture = echode_log::testing::capture();
    let mut host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin_dylib()),
        fault: Some("slow-save"),
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some("dev.echode.test-state"),
    })
    .expect("state plugin");
    let mut control = host.control_handle();
    control.restore_state(&[128]).expect("make one change");
    // The helper publishes the load's dirty notice before this next reply.
    let _ = control.parameters();
    let deadline = Instant::now() + Duration::from_secs(12);
    let saved = loop {
        if let Some(bytes) = control.poll_state_save() {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "the background capture was not retried after the helper's timeout"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(saved, [128]);
    assert_eq!(control.applied_state().unwrap().bytes(), [128]);
    assert!(
        capture.on_this_thread().iter().any(|record| {
            record["message"] == "background plugin state capture failed"
                && record["fields"]["retry"] == true
                && record["fields"]["cause"]
                    .as_str()
                    .is_some_and(|cause| cause.contains("TimedOut(MainCall("))
        }),
        "the fixture must actually cross the helper's timeout"
    );

    // The fixture's 64-frame buffers do not limit product block sizes.
    let input = stereo(64, 0.0);
    let mut output = stereo(64, 0.0);
    host.process(
        &[&input[0], &input[1]],
        &mut outs(&mut output),
        Duration::from_secs(2),
    )
    .expect("processing after the capture");
    assert!(
        output
            .iter()
            .flatten()
            .all(|sample| (*sample - 0.5).abs() < 1.0e-4)
    );
}
