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
fn audio_config_presets_select_ids_and_exchange_exact_ports_under_rtsan() {
    use echode_sandbox::PluginFailure;
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    wait_probe(&mut host, [3, 1, 2, 0]);
    let configs = host
        .control_handle()
        .audio_port_configs()
        .expect("audio configuration presets")
        .configs;
    assert_eq!(
        configs.iter().map(|config| config.id).collect::<Vec<_>>(),
        [100, 206, 303]
    );
    assert!(configs.iter().all(|config| config.name == "B"));
    assert_eq!(configs[1].main_output_channels, 1);
    assert_eq!(configs[2].input_ports, 2);
    for (id, ports) in [
        (206, PortLayout::new(vec![2], vec![1])),
        (303, PortLayout::new(vec![1, 1], vec![2])),
        (100, PortLayout::new(vec![2], vec![2])),
    ] {
        host.control_handle()
            .select_audio_port_config(id)
            .expect("select preset by id");
        assert_eq!(
            host.control_handle().audio_ports().1,
            ports,
            "configuration reply preceded its port notice"
        );
        assert!(
            process_ports(&mut host, 32)
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0),
            "audio configuration processed a stale mapping"
        );
        assert!(host.follow_ports().unwrap());
        for frames in [1, 32, 127, 256] {
            let output = process_ports(&mut host, frames);
            for (channel, plane) in output.iter().enumerate() {
                let expected = if id == 303 && channel == 1 {
                    0.0
                } else {
                    0.25 * (channel + 1) as f32
                };
                assert!(
                    plane.iter().all(|sample| (*sample - expected).abs() < 1e-6),
                    "audio configuration {id} channel {channel}: {plane:?}"
                );
            }
        }
        let state = host.control_handle().capture_state().unwrap();
        assert_eq!(
            state[0] & 0x80,
            0,
            "audio configuration lifecycle: {state:?}"
        );
    }
    for id in [0, 1, u32::MAX] {
        assert!(matches!(
            host.control_handle().select_audio_port_config(id),
            Err(SessionError::Refused(Refusal::Plugin(
                PluginFailure::AudioConfigRefused
            )))
        ));
        check_audio(&mut host);
    }
    // Selection marks plugin-owned state dirty; the selected layout survives a
    // capture/restore through the same product state path as other plugin edits.
    let state = host.control_handle().capture_state().unwrap();
    host.control_handle().restore_state(&[6]).unwrap();
    wait_ports(&mut host, &PortLayout::new(vec![2], vec![1]));
    host.control_handle().restore_state(&state).unwrap();
    wait_ports(&mut host, &PortLayout::new(vec![2], vec![2]));
    check_audio(&mut host);
}

#[test]
fn missing_audio_config_extension_is_a_typed_refusal_and_audio_resumes_under_rtsan() {
    use echode_sandbox::{AudioPortRequest, PluginFailure};
    let mut host = spawn_plugin(None, SessionLayout::stereo());
    for result in [
        host.control_handle().audio_port_configs().map(|_| ()),
        host.control_handle().select_audio_port_config(100),
    ] {
        assert!(matches!(
            result,
            Err(SessionError::Refused(Refusal::Plugin(
                PluginFailure::NoAudioConfigs
            )))
        ));
    }
    let request = [AudioPortRequest {
        input: true,
        index: 0,
        channels: 1,
        details: None,
    }];
    for result in [
        host.control_handle()
            .can_apply_audio_port_configuration(&request)
            .map(|_| ()),
        host.control_handle()
            .apply_audio_port_configuration(&request),
    ] {
        assert!(matches!(
            result,
            Err(SessionError::Refused(Refusal::Plugin(
                PluginFailure::NoConfigurableAudioPorts
            )))
        ));
    }
    check_audio(&mut host);
}

#[test]
fn configurable_audio_ports_and_config_info_exchange_ports_under_rtsan() {
    use echode_sandbox::{AudioPortRequest, PluginFailure};
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    wait_probe(&mut host, [3, 1, 2, 0]);
    let initial = host
        .control_handle()
        .audio_port_configs()
        .expect("audio configuration presets and info");
    assert_eq!(initial.current_id, Some(100));
    assert_eq!(
        initial
            .configs
            .iter()
            .map(|config| &config.ports)
            .collect::<Vec<_>>(),
        [
            &Some(PortLayout::new(vec![2], vec![2])),
            &Some(PortLayout::new(vec![2], vec![1])),
            &Some(PortLayout::new(vec![1, 1], vec![2])),
        ]
    );
    host.control_handle()
        .select_audio_port_config(303)
        .expect("select split preset");
    assert!(host.follow_ports().unwrap());
    assert_eq!(
        host.control_handle()
            .audio_port_configs()
            .unwrap()
            .current_id,
        Some(303)
    );
    let unsupported = [AudioPortRequest {
        input: true,
        index: 0,
        channels: 6,
        details: None,
    }];
    assert!(
        !host
            .control_handle()
            .can_apply_audio_port_configuration(&unsupported)
            .expect("query unsupported configurable layout")
    );
    let mono_in = [
        AudioPortRequest {
            input: true,
            index: 1,
            channels: 0,
            details: None,
        },
        AudioPortRequest {
            input: true,
            index: 0,
            channels: 1,
            details: None,
        },
    ];
    assert!(
        host.control_handle()
            .can_apply_audio_port_configuration(&mono_in)
            .expect("query custom mono input configuration")
    );
    host.control_handle()
        .apply_audio_port_configuration(&mono_in)
        .expect("apply custom mono input configuration");
    let custom_ports = PortLayout::new(vec![1], vec![2]);
    assert_eq!(host.control_handle().audio_ports().1, custom_ports);
    assert!(
        process_ports(&mut host, 32)
            .iter()
            .flatten()
            .all(|sample| *sample == 0.0),
        "configurable ports processed a stale mapping"
    );
    assert!(host.follow_ports().unwrap());
    for frames in [1, 32, 127, 256] {
        let output = process_ports(&mut host, frames);
        assert!(output[0].iter().all(|s| (*s - 0.25).abs() < 1e-6));
        assert!(output[1].iter().all(|s| *s == 0.0));
    }
    assert_eq!(
        host.control_handle()
            .audio_port_configs()
            .unwrap()
            .current_id,
        None,
        "custom configurable port layout must clear preset current_config"
    );
    let mono_both = [AudioPortRequest {
        input: false,
        index: 0,
        channels: 1,
        details: Some(echode_sandbox::AudioPortDetails::Surround(vec![2])),
    }];
    host.control_handle()
        .apply_audio_port_configuration(&mono_both)
        .expect("apply mono input and mono output configuration");
    assert!(host.follow_ports().unwrap());
    for frames in [1, 32, 127, 256] {
        let output = process_ports(&mut host, frames);
        assert_eq!(output.len(), 1);
        assert!(output[0].iter().all(|s| (*s - 0.25).abs() < 1e-6));
    }
    assert!(matches!(
        host.control_handle()
            .apply_audio_port_configuration(&unsupported),
        Err(SessionError::Refused(Refusal::Plugin(
            PluginFailure::ConfigurableAudioPortsRefused
        )))
    ));
    let output = process_ports(&mut host, 32);
    assert_eq!(output.len(), 1);
    assert!(output[0].iter().all(|s| (*s - 0.25).abs() < 1e-6));
    let state = host.control_handle().capture_state().unwrap();
    assert_eq!(
        state[0] & 0x80,
        0,
        "configurable audio ports lifecycle probe failed: {state:?}"
    );
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

/// Waits until the parent has read the ports of the plugin's latest
/// activation, and returns their revision.
fn wait_ports(host: &mut PluginHost, expected: &PortLayout) -> u64 {
    // An eventual notice, not a performance measurement.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.poll_notice().expect("notices");
        let (revision, ports) = host.control_handle().audio_ports();
        if ports == *expected {
            return revision;
        }
        assert!(!host.has_exited(), "{}", host.stderr_log());
        assert!(
            Instant::now() < deadline,
            "the parent did not read {expected:?}: {ports:?}; {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// One block through the current mapping, each input channel at its own
/// level. Outputs start at 1.0, so a written silence is visible.
fn process_ports(host: &mut PluginHost, frames: usize) -> Vec<Vec<f32>> {
    let layout = host.layout();
    let input: Vec<Vec<f32>> = (0..layout.input_channels)
        .map(|channel| vec![0.125 * (channel + 1) as f32; frames])
        .collect();
    let mut output = vec![vec![1.0; frames]; layout.output_channels as usize];
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    host.process(&inputs, &mut outputs, Duration::from_secs(2))
        .expect("a block through the mapping");
    output
}

/// ADR 0382: a plugin that changes its channel counts or its port list while
/// inactive keeps running. The mapping routed for the old ports is silent
/// rather than processed in buffers of another shape, even when the flattened
/// sizes match, and a mapping routed for the new ports carries the plugin's
/// audio. Going back to the first ports is another change.
#[test]
fn dimension_changes_reach_the_parent_and_a_mapping_routed_for_them_under_rtsan() {
    let stereo = PortLayout::new(vec![2], vec![2]);
    for (layout, inputs, outputs, reads) in [
        (2u8, vec![1], vec![2], 4u8),
        (3, vec![1, 1], vec![2], 5),
        (6, vec![2], vec![1], 4),
    ] {
        let changed = PortLayout::new(inputs, outputs);
        let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
        wait_probe(&mut host, [3, 1, 2, 0]);
        let (first, ports) = host.control_handle().audio_ports();
        assert_eq!((host.ports(), &ports), (&stereo, &stereo));
        host.control_handle().restore_state(&[layout]).unwrap();
        let revision = wait_ports(&mut host, &changed);
        assert_ne!(revision, first, "mode {layout}");
        wait_probe(&mut host, [7, 2, reads, layout]);
        assert_eq!(
            host.ports(),
            &stereo,
            "the parent remaps; the helper does not"
        );
        for plane in process_ports(&mut host, 32) {
            assert!(
                plane.iter().all(|sample| *sample == 0.0),
                "mode {layout}: a mapping for the old ports reached process"
            );
        }
        assert!(host.follow_ports().expect("a mapping for the new ports"));
        assert_eq!(host.ports(), &changed);
        assert_eq!(
            (host.layout().input_channels, host.layout().output_channels),
            (
                changed.input_channels().unwrap(),
                changed.output_channels().unwrap()
            )
        );
        assert!(!host.follow_ports().unwrap(), "the mapping is current");
        for frames in [1, 32, 127, 256] {
            // The fixture doubles the input paired with each output channel,
            // and the second output channel of a one-input port has none.
            let output = process_ports(&mut host, frames);
            for (channel, plane) in output.iter().enumerate() {
                let expected = if channel == 0 { 0.25 } else { 0.0 };
                assert!(
                    plane
                        .iter()
                        .all(|sample| (*sample - expected).abs() < 1.0e-6),
                    "mode {layout}, channel {channel}: {plane:?}"
                );
            }
        }
        let log = host.stderr_log();
        assert!(
            !log.contains("activate failed") && !log.contains("activate after restart failed"),
            "{log}"
        );
        if layout == 6 {
            host.control_handle().restore_state(&[0]).unwrap();
            assert_ne!(wait_ports(&mut host, &stereo), revision);
            for plane in process_ports(&mut host, 32) {
                assert!(plane.iter().all(|sample| *sample == 0.0));
            }
            assert!(host.follow_ports().unwrap());
            assert_eq!(host.ports(), &stereo);
            check_audio(&mut host);
        }
    }
}

/// Port ids are not routing. A plugin that renumbers its ports keeps its
/// mapping, its processor's audio, and its ports revision (ADR 0382).
#[test]
fn port_identity_changes_keep_the_mapping_under_rtsan() {
    for layout in [4u8, 8] {
        let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
        wait_probe(&mut host, [3, 1, 2, 0]);
        let (revision, ports) = host.control_handle().audio_ports();
        host.control_handle().restore_state(&[layout]).unwrap();
        wait_probe(&mut host, [7, 2, 4, layout]);
        check_audio(&mut host);
        host.poll_notice().expect("notices");
        assert_eq!(host.control_handle().audio_ports(), (revision, ports));
        assert!(!host.follow_ports().unwrap());
    }
}

/// A mapping must hold exactly the ports it is routed for; the parent refuses
/// another before asking the helper, and the current mapping keeps playing.
#[test]
fn a_mapping_that_does_not_hold_its_ports_is_refused_under_rtsan() {
    let mut host = spawn_plugin(Some("audio-ports"), SessionLayout::stereo());
    wait_probe(&mut host, [3, 1, 2, 0]);
    let refused =
        host.prepare_mapping_for(SessionLayout::stereo(), PortLayout::new(vec![2], vec![1]));
    assert!(
        matches!(
            refused,
            Err(SessionError::Protocol(ProtocolError::PortsOutsideLayout))
        ),
        "{refused:?}"
    );
    assert!(!host.mapping_pending());
    check_audio(&mut host);
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
