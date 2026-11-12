use super::*;

#[test]
fn plugin_latency_is_reported_at_startup_and_after_restart() {
    // This fixture uses a 64-frame block only to drive one restart request.
    let mut host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin_dylib()),
        fault: Some("latency"),
        workgroup: None,
        layout: SessionLayout {
            max_frames: 64,
            ..SessionLayout::stereo()
        },
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: None,
    })
    .unwrap();
    let read_latency = |host: &mut PluginHost| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(PluginNotice::PluginRestarted { latency_samples }) =
                host.poll_notice().unwrap()
            {
                break latency_samples;
            }
            assert!(
                Instant::now() < deadline,
                "plugin latency notice was not delivered"
            );
            std::thread::yield_now();
        }
    };
    let control = host.control_handle();
    assert_eq!(
        control.latency_samples(),
        256,
        "startup latency must not be hard-coded to zero"
    );
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    host.process(
        &[&[0.25; 64], &[0.25; 64]],
        &mut [&mut left, &mut right],
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        read_latency(&mut host),
        512,
        "restart must refresh latency on the main thread"
    );
    assert_eq!(control.latency_samples(), 512);
    assert_eq!(control.take_activity().unwrap().latency_samples, Some(512));
    host.process(
        &[&[0.25; 64], &[0.25; 64]],
        &mut [&mut left, &mut right],
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        read_latency(&mut host),
        0,
        "zero latency must replace the old value"
    );
    assert_eq!(control.latency_samples(), 0);
    assert_eq!(control.take_activity().unwrap().latency_samples, Some(0));
}

#[test]
fn plugins_without_latency_extension_report_zero() {
    let host = spawn_declared("dev.echode.test-gain").unwrap();
    assert_eq!(host.control_handle().latency_samples(), 0);
}
