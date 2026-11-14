use super::*;

#[test]
fn audio_port_name_rescans_keep_the_layout_and_processor_under_rtsan() {
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    // Capabilities and the main callback are checked without a window or any
    // audio request. This is an eventual lifecycle check, not a timing test.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = host.control_handle().capture_state().expect("port probe");
        assert_eq!(state.len(), 2);
        assert_eq!(
            state[0] & 0x80,
            0,
            "audio-port capability probe failed: {state:?}; {}",
            host.stderr_log()
        );
        if state[0] == 3 {
            assert_eq!(state[1], 1, "name rescan must not reactivate the plugin");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "audio-port main callback did not arrive: {state:?}; {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for frames in [1, 32, 127, 256] {
        // These buffers cover this fixture's block lengths, not a product limit.
        let input = stereo(frames, 0.1);
        let mut output = [vec![0.0; frames], vec![0.0; frames]];
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .expect("audio after name rescans");
        for (source, rendered) in input.iter().zip(&output) {
            for (source, rendered) in source.iter().zip(rendered) {
                assert!((rendered - source * 2.0).abs() < 1.0e-6);
            }
        }
    }
    assert_eq!(host.control_handle().capture_state().unwrap(), [3, 1]);
    while let Some(notice) = host.poll_notice().expect("notice") {
        assert!(
            !matches!(notice, PluginNotice::PluginRestarted { .. }),
            "name-only rescans must not request a parent graph rebuild"
        );
    }
}
