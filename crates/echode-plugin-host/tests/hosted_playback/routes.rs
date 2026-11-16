//! Routes exercise the product callback and isolated helpers, never a device.
use super::compensation::{assert_impulse, delay_route, impulses, play};
use super::*;

// These sizes bound only this impulse fixture, not the product graph.
const FRAMES: usize = 160;
const BLOCKS: [usize; 4] = [1, 7, 64, 512];

fn key_route() -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some("sidechain".into()),
        state: None,
    }
}

#[test]
fn sends_and_keys_meet_the_main_at_the_device_for_every_block_under_rtsan() {
    let (project, target) = Project::new().add_track("Return");
    let (project, source) = project.add_track("Key");
    let device = Id::generate();
    let source_delay = Id::generate();
    let project = project
        .add_clap_device(target, device, "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(source, source_delay, "dev.echode.test-ports")
        .unwrap()
        .add_send_with_id(Id::generate(), source, target, 0.5)
        .unwrap()
        .add_sidechain_with_id(Id::generate(), source, device, 0.25)
        .unwrap();
    // Both dependencies source -> target coexist. Main and key arrive at 57;
    // the source's own master contribution is not a key and must occur once.
    let routes = [
        key_route(),
        delay_route("dev.echode.test-ports", "37", false),
    ];
    for block in BLOCKS {
        let clips = impulses(vec![
            (source, 20, 0.25, -0.125),
            (target, 20, 0.125, 0.0625),
        ]);
        let (control, left, right) = play(&project, 48_000, block, &routes, clips, FRAMES);
        assert_eq!(control.latency_samples(), 37);
        assert_impulse(&left, 57, 0.75, "send + main + key must meet");
        assert_impulse(&right, 57, -0.25, "stereo key stays separate");
    }
}

fn gain(project: &Project, track: Id, value: f32) -> Project {
    let parameter = project
        .tracks()
        .iter()
        .find(|item| item.id == track)
        .unwrap()
        .devices
        .iter()
        .find(|device| device.kind == echode_model::DeviceKind::Mixer)
        .unwrap()
        .parameters
        .iter()
        .find(|param| param.key == echode_model::ParamKey::Gain)
        .unwrap()
        .id;
    project.set_parameter(parameter, value).unwrap()
}

#[test]
fn key_lines_and_downstream_main_lines_align_even_with_bypass_under_rtsan() {
    let (project, target) = Project::new().add_track("Main");
    let (project, slow) = project.add_track("Slow key");
    let (project, fast) = project.add_track("Fast key");
    let (pre, key, delay) = (Id::generate(), Id::generate(), Id::generate());
    let project = project
        .add_clap_device(target, pre, "dev.echode.test-state")
        .unwrap()
        .add_clap_device(target, key, "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(slow, delay, "dev.echode.test-ports")
        .unwrap()
        .add_sidechain_with_id(Id::generate(), slow, key, 0.25)
        .unwrap()
        .add_sidechain_with_id(Id::generate(), fast, key, 0.5)
        .unwrap();
    let project = gain(&gain(&project, slow, 0.0), fast, 0.0);
    let routes = [
        delay_route("dev.echode.test-state", "11", false),
        key_route(),
        delay_route("dev.echode.test-ports", "37", false),
    ];
    for bypassed in [false, true] {
        let case = project.set_device_bypassed(pre, bypassed).unwrap();
        for block in BLOCKS {
            let clips = impulses(vec![
                (target, 20, 0.125, -0.0625),
                (slow, 20, 0.25, 0.125),
                (fast, 20, 0.0625, -0.03125),
            ]);
            let (_, left, right) = play(&case, 48_000, block, &routes, clips, FRAMES);
            assert_impulse(&left, 57, 0.5, "main line and two key lines must align");
            assert_impulse(&right, 57, 0.0, "keys must retain polarity");
        }
    }
}

#[test]
fn a_missing_auxiliary_input_is_refused_without_replacing_the_sounding_plan() {
    let (project, target) = Project::new().add_track("Main");
    let (project, source) = project.add_track("Key");
    let device = Id::generate();
    let project = project
        .add_clap_device(target, device, "dev.echode.test-gain")
        .unwrap();
    let route = effect_route(&plugin_dylib());
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 64, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(stereo_clips(source.raw(), 0.125, -0.0625));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    let invalid = project
        .add_sidechain_with_id(Id::generate(), source, device, 1.0)
        .unwrap();
    assert!(
        matches!(control.replace(&invalid), Err(echode_engine::PlaybackError::SidechainMapping { device: id }) if id == device.raw())
    );
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().all(|&sample| sample == 0.125));
    assert!(right.iter().all(|&sample| sample == -0.0625));
    assert!(!control.status().failed);
}

#[test]
fn key_overflow_is_contained_before_the_helper_and_mutes_the_receiving_track_under_rtsan() {
    let (project, target) = Project::new().add_track("Main");
    let (project, source) = project.add_track("Key");
    let device = Id::generate();
    let project = project
        .add_clap_device(target, device, "dev.echode.test-gain")
        .unwrap()
        .add_sidechain_with_id(Id::generate(), source, device, 4.0)
        .unwrap();
    let project = gain(&project, source, 0.0);
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 64, &host_bin(), &[key_route()]).unwrap();
    control.set_clip_loader(stereo_clips(source.raw(), 2.0e38, -2.0e38));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().chain(&right).all(|&sample| sample == 0.0));
    let meter = control
        .meters()
        .into_iter()
        .find(|meter| meter.track == target.raw())
        .unwrap();
    assert_eq!(meter.auto_mute().unwrap().device, None);
    assert!(control.render_failure().is_some());
}

#[test]
fn routing_lines_survive_plan_adoption_and_clear_on_locate_under_rtsan() {
    let (project, target) = Project::new().add_track("Main");
    let (project, slow) = project.add_track("Slow key");
    let (project, fast) = project.add_track("Fast key");
    let (pre, key, delay) = (Id::generate(), Id::generate(), Id::generate());
    let project = project
        .add_clap_device(target, pre, "dev.echode.test-state")
        .unwrap()
        .add_clap_device(target, key, "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(slow, delay, "dev.echode.test-ports")
        .unwrap()
        .add_sidechain_with_id(Id::generate(), slow, key, 0.25)
        .unwrap()
        .add_sidechain_with_id(Id::generate(), fast, key, 0.5)
        .unwrap();
    let project = gain(&gain(&project, slow, 0.0), fast, 0.0);
    let routes = [
        delay_route("dev.echode.test-state", "11", false),
        key_route(),
        delay_route("dev.echode.test-ports", "37", false),
    ];
    for locate in [false, true] {
        for block in BLOCKS {
            let (mut control, mut audio) =
                playback_with_plugins(&project, 48_000, block, &host_bin(), &routes).unwrap();
            control.set_clip_loader(impulses(vec![
                (target, 20, 0.125, -0.0625),
                (slow, 20, 0.25, 0.125),
                (fast, 20, 0.0625, -0.03125),
            ]));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
            for start in (0..31).step_by(block) {
                let count = block.min(31 - start);
                render_content(&mut audio, &mut left[..count], &mut right[..count]);
            }
            control.replace(&project).unwrap();
            if locate {
                control.command(PlaybackCommand::Locate(100)).unwrap();
            }
            let mut heard = Vec::new();
            for start in (31..FRAMES).step_by(block) {
                let count = block.min(FRAMES - start);
                render_content(&mut audio, &mut left[..count], &mut right[..count]);
                heard.extend_from_slice(&left[..count]);
            }
            assert_impulse(
                &heard,
                if locate { FRAMES } else { 57 - 31 },
                if locate { 0.0 } else { 0.5 },
                "routing memory must follow plan adoption and reset",
            );
            assert!(!control.status().failed);
        }
    }
}

#[test]
fn a_mono_key_is_not_promoted_and_removing_it_clears_the_auxiliary_input_under_rtsan() {
    let (project, target) = Project::new().add_track("Main");
    let (project, source) = project.add_track("Mono key");
    let device = Id::generate();
    let route_id = Id::generate();
    let project = project
        .add_clap_device(target, device, "dev.echode.test-gain")
        .unwrap()
        .add_sidechain_with_id(route_id, source, device, 0.5)
        .unwrap();
    let project = gain(&project, source, 0.0);
    let mut route = key_route();
    route.fault = Some("sidechain-mono".into());
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 64, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(constant_clips(source.raw(), 0.125, 4096));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().all(|&sample| sample == 0.25));
    assert!(right.iter().all(|&sample| sample == 0.0));
    let removed = project.remove_sidechain(route_id).unwrap();
    control.replace(&removed).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(
        left.iter().chain(&right).all(|&sample| sample == 0.0),
        "removed keys must not remain in the helper input"
    );
    // Changing the source to stereo must be rejected, never silently downmixed.
    control.set_clip_loader(stereo_clips(source.raw(), 0.125, -0.0625));
    assert!(matches!(
        control.replace(&project),
        Err(echode_engine::PlaybackError::SidechainMapping { .. })
    ));
}

#[test]
fn a_send_line_waits_for_the_returns_children_under_rtsan() {
    let (project, target) = Project::new().add_track("Return");
    let (project, slow) = project.add_child(target, "Delayed child").unwrap();
    let (project, fast) = project.add_track("Fast send");
    let project = project
        .add_clap_device(slow, Id::generate(), "dev.echode.test-gain")
        .unwrap()
        .add_send_with_id(Id::generate(), fast, target, 0.5)
        .unwrap();
    let project = gain(&project, fast, 0.0);
    let routes = [delay_route("dev.echode.test-gain", "37", false)];
    for block in BLOCKS {
        let clips = impulses(vec![
            (target, 20, 0.0625, -0.03125),
            (slow, 20, 0.25, -0.125),
            (fast, 20, 0.25, -0.125),
        ]);
        let (_, left, right) = play(&project, 48_000, block, &routes, clips, FRAMES);
        assert_impulse(
            &left,
            57,
            0.4375,
            "send line meets delayed children and local clips",
        );
        assert_impulse(&right, 57, -0.21875, "send line stereo");
    }
}

#[test]
fn send_and_key_lines_cross_loop_seams_without_repeating_or_losing_audio_under_rtsan() {
    let (project, target) = Project::new().add_track("Return");
    let (project, source) = project.add_track("Key");
    let key = Id::generate();
    let project = project
        .add_clap_device(target, key, "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(source, Id::generate(), "dev.echode.test-ports")
        .unwrap()
        .add_send_with_id(Id::generate(), source, target, 0.5)
        .unwrap()
        .add_sidechain_with_id(Id::generate(), source, key, 0.25)
        .unwrap();
    let project = gain(&project, source, 0.0);
    let routes = [
        key_route(),
        delay_route("dev.echode.test-ports", "600", false),
    ];
    // Only this fixture's extent: its impulse is outside the source seam fades
    // and arrives in the next pass through the compensation lines.
    let (length, at, late) = (1200usize, 800i64, 600usize);
    for block in BLOCKS {
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, block, &host_bin(), &routes).unwrap();
        control.set_clip_loader(impulses(vec![
            (source, at, 0.25, -0.125),
            (target, at, 0.125, -0.0625),
        ]));
        control.replace(&project).unwrap();
        control
            .command(PlaybackCommand::SetLoop {
                start: 0,
                end: length as i64,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while control.status().pending_plan {
            assert!(
                Instant::now() < deadline,
                "routing loop plan was not adopted"
            );
            render_content(&mut audio, &mut [], &mut []);
            control.service();
        }
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        let mut heard = Vec::new();
        while heard.len() < length * 3 {
            let count = block.min(length * 3 - heard.len());
            render_content(&mut audio, &mut left[..count], &mut right[..count]);
            heard.extend_from_slice(&left[..count]);
            assert!(!control.status().failed);
        }
        for (frame, &sample) in heard.iter().enumerate() {
            let expected = if frame >= late && frame % length == (at as usize + late) % length {
                0.5
            } else {
                0.0
            };
            assert!(
                (sample - expected).abs() < 1.0e-6,
                "routing loop: block {block}, frame {frame}: {sample} != {expected}"
            );
        }
    }
}
