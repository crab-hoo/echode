//! Stopping the score keeps the live plugin graph and its output running.

use super::*;

fn live(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_live_gated_until(left, right, Instant::now() + Duration::from_secs(5), true);
}

#[test]
fn contract_stopped_plugins_release_notes_and_keep_tails_under_rtsan() {
    let path = plugin_dylib();
    // These block lengths and chains bound the regression, not the engine.
    for frames in [7, 64, 512] {
        for chain in 0..3 {
            for command in [PlaybackCommand::Stop, PlaybackCommand::Pause] {
                let (_, _, _, mut control, mut audio) = tail_player(&path, frames, chain, "tail");
                let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                control.command(PlaybackCommand::Play).unwrap();
                live(&mut audio, &mut left, &mut right);
                assert!(left.iter().any(|value| *value != 0.0));
                let stopped = control.status().sample;
                control.command(command).unwrap();
                // At 48 kHz this is beyond the old 5 ms master mute, but well
                // inside the fixture's release/delay tail.
                for _ in 0..512usize.div_ceil(frames) {
                    live(&mut audio, &mut left, &mut right);
                    assert_eq!(control.status().sample, stopped);
                    assert!(!control.status().playing);
                    assert!(!audio.cleanup_pending(), "stop reset a live plugin");
                    assert!(
                        left.iter().any(|value| *value != 0.0),
                        "stop cut the tail: {frames}/{chain}"
                    );
                }
                // A held note must actually receive its off, rather than
                // sounding indefinitely while the transport clock is held.
                let drain = 8192usize + control.tail_samples() as usize;
                for _ in 0..drain.div_ceil(frames) {
                    live(&mut audio, &mut left, &mut right);
                }
                assert!(
                    left.iter().chain(&right).all(|value| *value == 0.0),
                    "held note survived: {frames}/{chain}: {:?}",
                    &left[..left.len().min(4)]
                );
                assert_eq!(control.status().sample, stopped);
                assert_eq!(control.status().plugin_missed_blocks, 0);
            }
        }
    }
}

#[test]
fn contract_a_stop_that_returns_to_the_start_keeps_tails_under_rtsan() {
    let path = plugin_dylib();
    // These block lengths and chains bound the regression, not the engine.
    for frames in [7, 64, 512] {
        for chain in 0..3 {
            let (_, _, _, mut control, mut audio) = tail_player(&path, frames, chain, "tail");
            let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
            control.command(PlaybackCommand::Play).unwrap();
            for _ in 0..1024usize.div_ceil(frames) {
                live(&mut audio, &mut left, &mut right);
            }
            assert!(left.iter().any(|value| *value != 0.0));
            assert!(control.status().sample > 0);
            // The session's stop: pause, then return to the remembered start.
            control.command(PlaybackCommand::Pause).unwrap();
            control.command(PlaybackCommand::Rewind(0)).unwrap();
            for _ in 0..512usize.div_ceil(frames) {
                live(&mut audio, &mut left, &mut right);
                assert_eq!(control.status().sample, 0, "stop did not return");
                assert!(!control.status().playing);
                assert!(!audio.cleanup_pending(), "the return reset a live plugin");
                assert!(
                    left.iter().any(|value| *value != 0.0),
                    "the return cut the tail: {frames}/{chain}"
                );
            }
            let drain = 8192usize + control.tail_samples() as usize;
            for _ in 0..drain.div_ceil(frames) {
                live(&mut audio, &mut left, &mut right);
            }
            assert!(
                left.iter().chain(&right).all(|value| *value == 0.0),
                "held note survived the return: {frames}/{chain}"
            );
            // Playing again starts at the start and sounds its note once more.
            control.command(PlaybackCommand::Play).unwrap();
            live(&mut audio, &mut left, &mut right);
            assert!(
                left.iter().any(|value| *value != 0.0),
                "the start did not sound again: {frames}/{chain}"
            );
            assert_eq!(control.status().sample, frames as i64);
            assert_eq!(control.status().plugin_missed_blocks, 0);
        }
    }
}

#[test]
fn contract_stopped_graph_outputs_autonomous_plugins_under_rtsan() {
    // The state fixture produces a constant without notes or audio input,
    // like a plugin's own keyboard. Its downstream effect must also process.
    let (project, track) = Project::new().add_track("autonomous plugin");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-state")
        .unwrap()
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let routes = [
        PluginRoute {
            clap_id: "dev.echode.test-state".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: None,
            state: Some(echode_engine::PluginState::from_bytes(vec![64])),
        },
        PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: None,
            state: None,
        },
    ];
    // Fixture-only callback lengths. The state change must reach every block.
    for frames in [1, 7, 64, 512] {
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, frames, &host_bin(), &routes).unwrap();
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        live(&mut audio, &mut left, &mut right);
        assert!(
            left.iter().chain(&right).all(|value| *value == 0.5),
            "stopped plugins were not processed"
        );
        assert_eq!(control.status().sample, 0);
        assert!(!control.status().playing);
        control.command(PlaybackCommand::Play).unwrap();
        live(&mut audio, &mut left, &mut right);
        let stopped = control.status().sample;
        control.command(PlaybackCommand::Stop).unwrap();
        for _ in 0..256usize.div_ceil(frames) {
            live(&mut audio, &mut left, &mut right);
        }
        control.restore_device_state(device.raw(), &[128]).unwrap();
        live(&mut audio, &mut left, &mut right);
        assert!(
            left.iter().chain(&right).all(|value| *value == 1.0),
            "stopped plugin output was muted"
        );
        assert_eq!(control.status().sample, stopped);
        assert!(!control.status().playing);
        assert_eq!(control.status().plugin_missed_blocks, 0);
        // The stopped graph is audible: its fader must retain the same 5 ms
        // ramp as playback instead of jumping to the new gain at the boundary.
        control.preview_fader(track.raw(), 0.5, 0.0).unwrap();
        live(&mut audio, &mut left, &mut right);
        let first = 1.0 - 0.5 / (48_000.0 / 200.0);
        assert!((left[0] - first).abs() < 1.0e-7, "stopped fader jumped");
        for _ in 0..256usize.div_ceil(frames) {
            live(&mut audio, &mut left, &mut right);
        }
        assert!(left.iter().chain(&right).all(|value| *value == 0.5));
        assert_eq!(control.status().sample, stopped);
    }
}
