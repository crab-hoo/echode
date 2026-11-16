//! Plugins that change their audio ports while playing (ADR 0382).
//!
//! `audio-ports:<layout>` restarts the gain fixture into another layout after
//! its first block. The fixture doubles the input paired with each output
//! channel, so the heard level tells which routing processed a block.
use super::*;

const HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn ports_route(layout: u8) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some(format!("audio-ports:{layout}")),
        state: None,
    }
}

/// Long enough to cover the change on this machine's helpers; shorter only
/// leaves fewer blocks to check. Sizes this fixture, not the engine.
const CLIP: usize = 480_000;

fn near(heard: f32, expected: f32) -> bool {
    (heard - expected).abs() < 1.0e-5
}

/// A stereo effect becomes stereo-in, mono-out after its first block. Until a
/// plan routes the track for that, the helper keeps the old mapping silent;
/// then the mapping and the routing change together, and the mono output is
/// heard at equal power on both sides. No block mixes the two routings.
#[derive(Clone, Copy)]
enum PortAction {
    Restart,
    SelectPreset,
    ConfigurePorts,
}

#[test]
fn a_port_change_while_playing_moves_the_mapping_and_the_routing_together_under_rtsan() {
    check_port_change(PortAction::Restart);
}

#[test]
fn an_audio_config_selection_uses_the_product_port_exchange_under_rtsan() {
    check_port_change(PortAction::SelectPreset);
}

#[test]
fn a_configurable_audio_port_change_uses_the_product_port_exchange_under_rtsan() {
    check_port_change(PortAction::ConfigurePorts);
}

fn check_port_change(action: PortAction) {
    let device = Id::generate();
    let (project, track) = Project::new().add_track("changing ports");
    let project = project
        .add_clap_device(track, device, "dev.echode.test-gain")
        .unwrap();
    let block = 64; // This test's scratch size, not an engine limit.
    let initial_layout = match action {
        PortAction::Restart => 6,
        PortAction::SelectPreset | PortAction::ConfigurePorts => 0,
    };
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        block,
        &host_bin(),
        &[ports_route(initial_layout)],
    )
    .unwrap();
    control.set_clip_loader(stereo_clips_of(track.raw(), 0.25, 0.125, CLIP));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    render_content(&mut audio, &mut left, &mut right);
    assert_close(&left, &vec![0.5; block]);
    assert_close(&right, &vec![0.25; block]);
    let before = control.compensation_changes();
    match action {
        PortAction::Restart => {}
        PortAction::SelectPreset => {
            let plugin = control.device_control_handle(device.raw()).unwrap();
            let configs = plugin
                .audio_port_configs()
                .expect("product configuration list")
                .configs;
            assert_eq!(configs[1].id, 206);
            plugin
                .select_audio_port_config(configs[1].id)
                .expect("product configuration selection");
        }
        PortAction::ConfigurePorts => {
            let plugin = control.device_control_handle(device.raw()).unwrap();
            assert_eq!(
                plugin
                    .audio_port_configs()
                    .expect("product configuration list")
                    .current_id,
                Some(100)
            );
            let request = [echode_sandbox::AudioPortRequest {
                input: false,
                index: 0,
                channels: 1,
            }];
            assert!(
                plugin
                    .can_apply_audio_port_configuration(&request)
                    .expect("query product configurable layout")
            );
            plugin
                .apply_audio_port_configuration(&request)
                .expect("apply product configurable layout");
            assert_eq!(
                plugin
                    .audio_port_configs()
                    .expect("updated configuration list")
                    .current_id,
                None
            );
        }
    }
    // A hung-helper guard, not a latency threshold.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "the new ports were not routed");
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert!(control.take_preparation_error().is_none());
        assert!(!control.status().failed);
        let frames = left.iter().zip(&right);
        // The gap is the helper's silence, from its restart until the
        // callback adopts the new mapping.
        let old = frames.clone().all(|(l, r)| near(*l, 0.5) && near(*r, 0.25));
        let gap = frames.clone().all(|(l, r)| *l == 0.0 && *r == 0.0);
        let new = frames
            .clone()
            .all(|(l, r)| near(*l, 0.5 * HALF) && near(*r, 0.5 * HALF));
        assert!(
            old || gap || new,
            "a block mixed routings: {left:?} {right:?}"
        );
        if new {
            break;
        }
    }
    for _ in 0..4 {
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert_close(&left, &vec![0.5 * HALF; block]);
        assert_close(&right, &vec![0.5 * HALF; block]);
    }
    assert!(control.latency_settled());
    assert_eq!(
        control.compensation_changes(),
        before + 1,
        "an export restarts when the routing changes"
    );
    assert!(control.plugin_failure().is_none());
}

/// A plugin that picks ports the track cannot route is refused with the bus
/// mapping error, as a device with those ports would be at opening. The old
/// plan keeps playing, the device is silent instead of guessed, and the
/// transport does not fail.
#[test]
fn an_unroutable_port_change_is_refused_and_the_device_is_silent_under_rtsan() {
    let (project, track) = Project::new().add_track("unroutable ports");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let block = 64; // This test's scratch size, not an engine limit.
    // One input channel cannot hear a stereo effect bus.
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, block, &host_bin(), &[ports_route(2)]).unwrap();
    control.set_clip_loader(stereo_clips_of(track.raw(), 0.25, 0.125, CLIP));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    render_content(&mut audio, &mut left, &mut right);
    assert_close(&left, &vec![0.5; block]);
    // A hung-helper guard, not a latency threshold.
    let deadline = Instant::now() + Duration::from_secs(10);
    let refused = loop {
        assert!(Instant::now() < deadline, "the change was not refused");
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        let frames = left.iter().zip(&right);
        assert!(
            frames.clone().all(|(l, r)| near(*l, 0.5) && near(*r, 0.25))
                || frames.clone().all(|(l, r)| *l == 0.0 && *r == 0.0),
            "a block was heard through a guessed routing: {left:?} {right:?}"
        );
        if let Some(error) = control.take_preparation_error() {
            break error;
        }
    };
    assert!(refused.to_string().contains("bus mapping"), "{refused}");
    for _ in 0..4 {
        render_content(&mut audio, &mut left, &mut right);
        assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    }
    assert!(!control.status().failed);
}
