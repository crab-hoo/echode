use super::*;
use echode_sandbox::{ParamChange, ParamEventKind, PluginActivity};

#[test]
fn parameter_description_rescans_during_relay_reads_keep_the_helper_alive_under_rtsan() {
    let plugin = plugin_dylib();
    let mut host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin),
        plugin_id: Some("dev.echode.test-params"),
        fault: Some("params-rescan"),
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 48_000,
        workgroup: None,
    })
    .expect("parameter fixture");
    let control = host.control_handle();
    for value in [0.5, 1.5] {
        control
            .set_parameters(&[ParamChange { id: 1, value }])
            .unwrap_or_else(|error| {
                panic!(
                    "parameter relay failed during description rescan: {error}; {}",
                    host.stderr_log()
                )
            });
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut activity = PluginActivity::default();
        loop {
            let infos = control.parameters().unwrap_or_else(|error| {
                panic!(
                    "parameter relay failed during description rescan: {error}; {}",
                    host.stderr_log()
                )
            });
            assert_eq!(infos[0].value, value);
            if let Some(next) = control.take_activity() {
                activity.merge(next);
            }
            if activity.relay.rescan && !activity.relay.events.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "description rescan/value relay did not arrive: {activity:?}; {}",
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!activity.relay.lost);
        assert_eq!(activity.relay.events.len(), 1);
        assert_eq!(activity.relay.events[0].id, 1);
        assert_eq!(activity.relay.events[0].kind, ParamEventKind::Value);
        assert_eq!(activity.relay.values[0].value, value);
    }
    // These buffers cover only this fixture's small post-notification block.
    let input = stereo(32, 0.1);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    host.process(
        &[&input[0], &input[1]],
        &mut outs(&mut output),
        Duration::from_secs(2),
    )
    .expect("audio after reentrant rescans");
    for (source, rendered) in input.iter().zip(&output) {
        for (source, rendered) in source.iter().zip(rendered) {
            assert!((rendered - source * 1.5).abs() < 1.0e-6);
        }
    }
}
