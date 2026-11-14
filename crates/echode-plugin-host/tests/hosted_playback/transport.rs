//! Musical time must reach every stage, including folder effects.
use super::*;
use echode_model::{Meter, Tempo};

fn fixture() -> Project {
    let base = Project::with_signature(
        Meter::try_new(7, 8).unwrap(),
        Tempo::rational(120, 1).unwrap(),
    )
    .with_tempo(Tick::new(147), Tempo::rational(60, 1).unwrap());
    let (project, parent) = base.add_track("folder");
    let (project, child) = project.add_child(parent, "child").unwrap();
    let (mut project, root) = project.add_track("root");
    for track in [child, child, parent, root] {
        project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
    }
    project
}

fn route() -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some("timeline".into()),
        state: None,
    }
}

#[test]
fn every_stage_receives_sample_accurate_transport_across_blocks_under_rtsan() {
    let project = fixture();
    // These block shapes and the short loop bound this regression only.
    for block in [1, 7, 64, 127] {
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, block, &host_bin(), &[route()]).unwrap();
        control.command(PlaybackCommand::Locate(-11)).unwrap();
        control
            .command(PlaybackCommand::SetLoop { start: -5, end: 25 })
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut position = -11;
        for _ in 0..3 {
            let mut left = vec![0.0; block];
            let mut right = left.clone();
            render_content(&mut audio, &mut left, &mut right);
            assert!(
                !control.status().failed,
                "transport render failed for block {block}"
            );
            for (left, right) in left.into_iter().zip(right) {
                if position == 25 {
                    position = -5;
                }
                let (beats, bpm) = if position < 5 {
                    (position as f64 / 24_000.0, 120.0)
                } else {
                    (1.0 / 4800.0 + (position - 5) as f64 / 48_000.0, 60.0)
                };
                let expected = (4.0 * beats / 16.0) as f32;
                assert!(
                    (left - expected).abs() < 2.0e-8,
                    "engine transport missing or block-dependent: block={block}, position={position}, left={left}, expected={expected}"
                );
                assert!(
                    (right - (4.0 * (1.0 + bpm / 1024.0)) as f32).abs() < 1.0e-6,
                    "tempo/playing state did not reach every stage: block={block}, position={position}, right={right}"
                );
                position += 1;
            }
        }
        control.command(PlaybackCommand::Pause).unwrap();
        let mut left = vec![0.0; block];
        let mut right = left.clone();
        // Stopped live processing still carries the held musical position.
        audio.render_live_gated_until(
            &mut left,
            &mut right,
            Instant::now() + Duration::from_secs(10),
            true,
        );
        let expected_bpm = if control.status().sample < 5 {
            120.0
        } else {
            60.0
        };
        assert!(
            right
                .iter()
                .all(|sample| (*sample - (4.0 * expected_bpm / 1024.0) as f32).abs() < 1.0e-6),
            "stopped transport did not reach every stage: {right:?}"
        );
        let held = control.status().sample;
        let held_beats = if held < 5 {
            held as f64 / 24_000.0
        } else {
            1.0 / 4800.0 + (held - 5) as f64 / 48_000.0
        };
        assert!(
            left.iter()
                .all(|sample| (*sample - (held_beats / 4.0) as f32).abs() < 2.0e-8),
            "stopped beat position advanced: {left:?}"
        );
        control.command(PlaybackCommand::ClearLoop).unwrap();
        let edited = project.with_tempo(Tick::new(0), Tempo::rational(90, 1).unwrap());
        control.replace(&edited).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while control.status().pending_plan {
            control.service();
            assert!(control.take_preparation_error().is_none());
            render_content(&mut audio, &mut left, &mut right);
            assert!(
                Instant::now() < deadline,
                "edited transport plan was not installed"
            );
        }
        control.command(PlaybackCommand::Locate(0)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        render_content(&mut audio, &mut left, &mut right);
        assert!(!control.status().failed, "edited transport render failed");
        assert!(
            left[0].abs() < 2.0e-8,
            "locate retained the old beat position"
        );
        assert!(
            (right[0] - 4.0 * (1.0 + 90.0 / 1024.0)).abs() < 1.0e-6,
            "edited tempo was not installed in every stage: {right:?}"
        );
    }
}
