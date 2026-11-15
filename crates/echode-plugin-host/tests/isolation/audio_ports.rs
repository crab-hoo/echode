use super::*;

fn wait_probe(host: &mut PluginHost, expected: [u8; 4]) {
    // Eventual lifecycle checks, not performance measurements. No window,
    // hardware, or audio request is needed to deliver main-thread callbacks.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = host.control_handle().capture_state().expect("port probe");
        assert_eq!(state.len(), 4);
        assert_eq!(
            state[0] & 0x80,
            0,
            "audio-port capability/lifecycle probe failed: {state:?}; {}",
            host.stderr_log()
        );
        if state == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "audio-port lifecycle did not converge: {state:?}; expected {expected:?}; {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check_audio(host: &mut PluginHost) {
    for frames in [1, 32, 127, 256] {
        // These buffers cover this fixture's block lengths, not a product limit.
        let input = stereo(frames, 0.1);
        let mut output = [vec![0.0; frames], vec![0.0; frames]];
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .expect("audio after port rescans");
        for (source, rendered) in input.iter().zip(&output) {
            for (source, rendered) in source.iter().zip(rendered) {
                assert!((rendered - source * 2.0).abs() < 1.0e-6);
            }
        }
    }
}

#[test]
fn audio_port_name_rescans_keep_the_layout_and_processor_under_rtsan() {
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    wait_probe(&mut host, [3, 1, 2, 0]);
    check_audio(&mut host);
    assert_eq!(host.control_handle().capture_state().unwrap(), [3, 1, 2, 0]);
    while let Some(notice) = host.poll_notice().expect("notice") {
        assert!(
            !matches!(notice, PluginNotice::PluginRestarted { .. }),
            "name-only rescans must not request a parent graph rebuild"
        );
    }
}

#[test]
fn dimension_preserving_audio_port_rescans_revalidate_before_activation_under_rtsan() {
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    wait_probe(&mut host, [3, 1, 2, 0]);
    // Change precision hints, port type, and in-place pairing together, then
    // revert. The helper still supplies separate f32 planes with the same shape.
    for (step, layout) in [1u8, 0, 1].into_iter().enumerate() {
        host.control_handle().restore_state(&[layout]).unwrap();
        let activations = step as u8 + 2;
        wait_probe(&mut host, [7, activations, activations * 2, layout]);
        assert_eq!(host.layout().input_channels, 2);
        assert_eq!(host.layout().output_channels, 2);
        check_audio(&mut host);
    }
}

#[test]
fn dimension_changes_are_rejected_before_reactivation_under_rtsan() {
    // The fixture deliberately violates the advertised LIST/CHANNEL_COUNT
    // restrictions, without sending an unsupported rescan flag. Even matching
    // flattened sizes must not let changed port boundaries or IDs reach process.
    for layout in [2u8, 3, 4, 6, 8] {
        let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
        wait_probe(&mut host, [3, 1, 2, 0]);
        host.control_handle().restore_state(&[layout]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !host.has_exited() {
            assert!(
                Instant::now() < deadline,
                "changed audio-port layout reached reactivation: mode {layout}; {}",
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // The stderr reader may still be draining after waitpid observes exit.
        while !host
            .stderr_log()
            .contains("plugin audio port layout changed")
        {
            assert!(Instant::now() < deadline, "{}", host.stderr_log());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !host.stderr_log().contains("audio-port-activation 2"),
            "a changed layout must be rejected before activate"
        );
        assert!(
            !host.stderr_log().contains("restart did not produce"),
            "the specific failure must be retained without a redundant fallback"
        );
    }
}

#[test]
fn invalid_audio_port_descriptions_after_deactivate_stop_the_helper_under_rtsan() {
    for (layout, reason) in [
        (5u8, "did not describe output port 0"),
        (7, "no audio ports"),
    ] {
        let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
        wait_probe(&mut host, [3, 1, 2, 0]);
        host.control_handle().restore_state(&[layout]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !host.has_exited() || !host.stderr_log().contains(reason) {
            assert!(
                Instant::now() < deadline,
                "invalid audio-port description was not rejected: mode {layout}; {}",
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
