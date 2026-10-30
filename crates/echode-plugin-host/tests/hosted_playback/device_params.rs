use super::*;
use echode_sandbox::{ParamChange, ParamText, ParamValue};

fn process_level(host: &mut PluginHost, input: f32) -> (f32, f32) {
    // One short block per check; this fixture length is not a host limit.
    let left_in = vec![input; 64];
    let right_in = vec![-input; 64];
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    host.process(
        &[&left_in, &right_in],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("process");
    (left[63], right[63])
}

#[test]
fn helper_lists_applies_and_saves_clap_parameters_under_rtsan() {
    let mut host = spawn_declared("dev.echode.test-params").expect("params plugin");
    let control = host.control_handle();
    // No block has been submitted yet. The processing thread flushes while idle.
    let applied = control
        .set_parameters(&[ParamChange { id: 1, value: 0.5 }])
        .expect("idle change");
    assert_eq!(
        applied,
        [ParamValue {
            id: 1,
            value: 0.5,
            text: "0.50x".into()
        }]
    );
    let infos = control.parameters().expect("parameters");
    let ids: Vec<_> = infos.iter().map(|info| info.id).collect();
    // The inverted range is skipped, not clamped into something editable.
    assert_eq!(ids, [1, 2, 3, 4]);
    assert_eq!(infos[0].name, "Gain");
    assert_eq!(infos[0].module, "Output");
    assert_eq!(
        (infos[0].min, infos[0].max, infos[0].default),
        (0.0, 2.0, 1.0)
    );
    assert_eq!((infos[0].value, infos[0].text.as_str()), (0.5, "0.50x"));
    assert!(infos[1].stepped() && infos[1].text == "Clean");
    assert!(infos[2].hidden() && infos[3].read_only());
    assert_eq!(process_level(&mut host, 0.5), (0.25, -0.25));

    let applied = control
        .set_parameters(&[
            ParamChange { id: 2, value: 1.0 },
            ParamChange { id: 1, value: 1.5 },
            ParamChange { id: 2, value: 1.0 },
        ])
        .expect("between blocks");
    assert_eq!(
        applied
            .iter()
            .map(|value| (value.id, value.text.as_str()))
            .collect::<Vec<_>>(),
        [(2, "Half"), (1, "1.50x")]
    );
    assert_eq!(process_level(&mut host, 0.5), (0.375, -0.375));

    let edited = control.capture_state().expect("capture");
    assert_eq!(edited.len(), 16);
    let mut defaults = Vec::from(1.0f64.to_le_bytes());
    defaults.extend(0.0f64.to_le_bytes());
    control.restore_state(&defaults).expect("restore defaults");
    assert_eq!(process_level(&mut host, 0.5), (0.5, -0.5));
    assert_eq!(control.parameters().unwrap()[0].value, 1.0);
    control.restore_state(&edited).expect("restore edit");
    assert_eq!(process_level(&mut host, 0.5), (0.375, -0.375));
    assert_eq!(control.parameters().unwrap()[1].text, "Half");

    let refused = control
        .set_parameters(&[ParamChange {
            id: u32::MAX,
            value: 0.0,
        }])
        .expect_err("invalid id");
    assert!(
        refused.to_string().contains("invalid parameter"),
        "{refused}"
    );
    assert!(
        control
            .set_parameters(&[ParamChange {
                id: 1,
                value: f64::NAN
            }])
            .is_err()
    );
    // The refused batches changed nothing.
    assert_eq!(control.parameters().unwrap()[0].value, 1.5);

    let gain = spawn_declared("dev.echode.test-gain").expect("gain");
    let plain = gain.control_handle();
    assert!(plain.parameters().expect("no extension").is_empty());
    let refused = plain
        .set_parameters(&[ParamChange { id: 1, value: 1.0 }])
        .expect_err("no params extension");
    assert!(refused.to_string().contains("parameter"), "{refused}");
}

/// Typed text reaches the plugin's `text_to_value` on its main thread
/// (ADR 0153). Reading text changes nothing; a plugin without a converter
/// says so, and the host then reads a plain number.
#[test]
fn contract_typed_text_reads_the_plugin_units_under_rtsan() {
    let host = spawn_declared("dev.echode.test-params").expect("params plugin");
    let control = host.control_handle();
    assert_eq!(
        control.parse_parameter(1, "0.75x").unwrap(),
        ParamText::Value(0.75)
    );
    assert_eq!(
        control.parse_parameter(2, "Mute").unwrap(),
        ParamText::Value(2.0)
    );
    for refused in ["loud", "1.0\0x", ""] {
        assert_eq!(
            control.parse_parameter(1, refused).unwrap(),
            ParamText::Refused,
            "{refused:?}"
        );
    }
    let infos = control.parameters().unwrap();
    assert_eq!((infos[0].value, infos[1].value), (1.0, 0.0));

    let gain = spawn_declared("dev.echode.test-gain").expect("gain");
    assert_eq!(
        gain.control_handle().parse_parameter(1, "1.0").unwrap(),
        ParamText::Unsupported
    );
}

pub(super) fn params_route(state: Option<Vec<u8>>) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-params".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
        state,
    }
}

pub(super) fn stereo_clip(track: u128) -> echode_engine::ClipLoader {
    // Long enough for every block below; not a playback limit.
    let left: Arc<[f32]> = vec![0.25; 48_000].into();
    let right: Arc<[f32]> = vec![-0.25; 48_000].into();
    Arc::new(move |_, _| {
        Ok(vec![LoadedClip {
            track,
            start_sample: 0,
            samples: Arc::clone(&left),
            right: Some(Arc::clone(&right)),
            fade_in: 0,
            fade_out: 0,
        }])
    })
}

pub(super) fn hear(
    control: &mut echode_engine::PlaybackControl,
    audio: &mut echode_engine::PlaybackAudio,
    frames: usize,
) -> (f32, f32) {
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    // Plan adoption takes a bounded number of fixture callbacks.
    for _ in 0..8 {
        render_content(audio, &mut left, &mut right);
        control.service();
        if !control.status().pending_plan {
            break;
        }
    }
    render_content(audio, &mut left, &mut right);
    assert!(!control.status().failed);
    assert!(control.plugin_failure().is_none());
    (left[frames - 1], right[frames - 1])
}

pub(super) fn close(heard: (f32, f32), level: f32) {
    assert!(
        (heard.0 - level).abs() < 1.0e-6 && (heard.1 + level).abs() < 1.0e-6,
        "{heard:?} != {level}"
    );
}

/// The product engine edits a running CLAP parameter, captures it as plugin
/// bytes, keeps the same helper when those bytes are recorded, and loads the
/// earlier bytes back for Undo without respawning. A recorded edit sounds the
/// same at every block length.
#[test]
fn contract_parameter_edit_sounds_records_and_restores_under_rtsan() {
    let (project, track) = Project::new().add_track("params");
    let device = Id::from_raw(0x9A);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let mut recorded = None;
    // Block lengths for this fixture only; the engine has no such limit.
    for frames in [7, 64, 512] {
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, frames, &host_bin(), &[params_route(None)])
                .unwrap();
        control.set_clip_loader(stereo_clip(track.raw()));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        close(hear(&mut control, &mut audio, frames), 0.25);
        let before = control
            .device_control_handle(device.raw())
            .unwrap()
            .capture_state()
            .unwrap();
        let applied = control
            .device_control_handle(device.raw())
            .unwrap()
            .set_parameters(&[
                echode_sandbox::ParamChange { id: 1, value: 1.5 },
                echode_sandbox::ParamChange { id: 2, value: 1.0 },
            ])
            .unwrap();
        assert_eq!(applied[0].text, "1.50x");
        close(hear(&mut control, &mut audio, frames), 0.1875);
        let edited = control
            .device_control_handle(device.raw())
            .unwrap()
            .capture_state()
            .unwrap();
        assert_ne!(edited, before);
        assert_eq!(
            control.device_applied_state(device.raw()).unwrap(),
            Some(edited.clone())
        );

        // Recording the captured bytes replaces the plan but not the helper.
        // The hidden value is not in the fixture's state, so a respawn resets it.
        control
            .device_control_handle(device.raw())
            .unwrap()
            .set_parameters(&[echode_sandbox::ParamChange { id: 3, value: 0.75 }])
            .unwrap();
        let host = host_bin();
        let saved = project
            .set_device_state(device, Some(echode_history::object_hash(&edited)))
            .unwrap();
        control.set_routes(&host, &[params_route(Some(edited.clone())), sine_route()]);
        control.replace(&saved).unwrap();
        close(hear(&mut control, &mut audio, frames), 0.1875);
        let kept = control
            .device_control_handle(device.raw())
            .unwrap()
            .parameters()
            .unwrap();
        assert_eq!((kept[1].text.as_str(), kept[2].value), ("Half", 0.75));

        // Undo loads the earlier bytes into the same running helper.
        control.restore_device_state(device.raw(), &before).unwrap();
        close(hear(&mut control, &mut audio, frames), 0.25);
        assert_eq!(
            control
                .device_control_handle(device.raw())
                .unwrap()
                .parameters()
                .unwrap()[2]
                .value,
            0.75
        );
        control.restore_device_state(device.raw(), &edited).unwrap();
        close(hear(&mut control, &mut audio, frames), 0.1875);
        assert!(
            control
                .device_control_handle(Id::from_raw(0x9B).raw())
                .is_err()
        );
        if let Some(previous) = &recorded {
            assert_eq!(previous, &edited, "saved bytes depend on block length");
        }
        recorded = Some(edited);

        // A fresh engine with the recorded bytes sounds like the edit.
        let (mut reopened, mut reopened_audio) = playback_with_plugins(
            &saved,
            48_000,
            frames,
            &host,
            &[params_route(recorded.clone())],
        )
        .unwrap();
        reopened.set_clip_loader(stereo_clip(track.raw()));
        reopened.replace(&saved).unwrap();
        reopened.command(PlaybackCommand::Play).unwrap();
        close(hear(&mut reopened, &mut reopened_audio, frames), 0.1875);
    }
}

/// Undo and Redo of a recorded state reach the running helper. Bytes the
/// helper already holds are not loaded again; defaults, unreadable objects
/// and refused bytes ask the caller to replace the graph.
#[test]
fn recorded_states_load_into_the_running_helper_or_request_replacement_under_rtsan() {
    let (project, track) = Project::new().add_track("params");
    let device = Id::from_raw(0x9C);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let frames = 64;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[params_route(None)])
            .unwrap();
    control.set_clip_loader(stereo_clip(track.raw()));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let before = control
        .device_control_handle(device.raw())
        .unwrap()
        .capture_state()
        .unwrap();
    control
        .device_control_handle(device.raw())
        .unwrap()
        .set_parameters(&[
            echode_sandbox::ParamChange { id: 2, value: 2.0 },
            echode_sandbox::ParamChange { id: 3, value: 0.5 },
        ])
        .unwrap();
    let edited = control
        .device_control_handle(device.raw())
        .unwrap()
        .capture_state()
        .unwrap();
    close(hear(&mut control, &mut audio, frames), 0.0);

    let id = device.raw();
    assert!(!control.restore_recorded_states(&[id], |_| Ok(Some(edited.clone()))));
    assert!(!control.restore_recorded_states(&[id], |_| Ok(Some(before.clone()))));
    close(hear(&mut control, &mut audio, frames), 0.25);
    // Same helper: the unsaved hidden value survived the load.
    assert_eq!(
        control
            .device_control_handle(id)
            .unwrap()
            .parameters()
            .unwrap()[2]
            .value,
        0.5
    );
    assert!(!control.restore_recorded_states(&[0x9D], |_| Ok(None)));
    assert!(control.restore_recorded_states(&[id], |_| Ok(None)));
    assert!(control.restore_recorded_states(&[id], |_| Err("missing object".into())));
    assert!(control.restore_recorded_states(&[id], |_| Ok(Some(vec![1, 2, 3]))));
    // A refused load leaves the plugin as it was.
    close(hear(&mut control, &mut audio, frames), 0.25);
}

/// Recorded parameter values are plugin bytes, so they sound the same at
/// every block length: the waveform is compared sample by sample.
#[test]
fn recorded_parameter_state_is_independent_of_block_size_under_rtsan() {
    let (project, track) = Project::new().add_track("params");
    let device = Id::from_raw(0x9E);
    let mut state = Vec::from(1.5f64.to_le_bytes());
    state.extend(1.0f64.to_le_bytes());
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap()
        .set_device_state(device, Some(echode_history::object_hash(&state)))
        .unwrap();
    // This length and these block sizes bound only this comparison.
    let input: Arc<[f32]> = (0..1537)
        .map(|index| (index as f32 * 0.031).sin() * 0.25)
        .collect::<Vec<_>>()
        .into();
    let other: Arc<[f32]> = input
        .iter()
        .map(|sample| -sample)
        .collect::<Vec<_>>()
        .into();
    let mut reference = None;
    for block in [1usize, 7, 64, 512] {
        let (mut control, mut audio) = playback_with_plugins(
            &project,
            48_000,
            block,
            &host_bin(),
            &[params_route(Some(state.clone()))],
        )
        .unwrap();
        let (left, right) = (Arc::clone(&input), Arc::clone(&other));
        control.set_clip_loader(Arc::new(move |_, _| {
            Ok(vec![LoadedClip {
                track: track.raw(),
                start_sample: 0,
                samples: Arc::clone(&left),
                right: Some(Arc::clone(&right)),
                fade_in: 0,
                fade_out: 0,
            }])
        }));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = vec![0.0; block];
        let mut right = left.clone();
        let mut heard = (Vec::new(), Vec::new());
        for offset in (0..input.len()).step_by(block) {
            let count = block.min(input.len() - offset);
            render_content(&mut audio, &mut left[..count], &mut right[..count]);
            heard.0.extend_from_slice(&left[..count]);
            heard.1.extend_from_slice(&right[..count]);
        }
        assert!(!control.status().failed && control.plugin_failure().is_none());
        for (heard, source) in heard.0.iter().zip(input.iter()) {
            assert!(
                (heard - source * 0.75).abs() < 1.0e-6,
                "{heard} != {}",
                source * 0.75
            );
        }
        if let Some(reference) = &reference {
            assert_eq!(&heard, reference, "block {block}");
        } else {
            reference = Some(heard);
        }
    }
}

/// A device view waits for a helper's reply on its own thread. While the
/// plugin holds a parameter listing, the control thread still polls
/// background saves, replaces the graph for an unrelated edit and reads the
/// applied bytes, so a slow plugin does not hold the document worker.
#[test]
fn contract_held_parameter_listing_does_not_hold_the_control_thread() {
    use std::io::{Read, Write};
    let (project, track) = Project::new().add_track("params");
    let device = Id::from_raw(0x9E);
    let project = project
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    // A short name: Unix socket paths have a small fixed limit on macOS.
    let socket = std::env::temp_dir().join(format!("echode-hold-{}", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut route = params_route(None);
    route.fault = Some(format!("params-wait:{}", socket.display()));
    let frames = 64;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.set_clip_loader(stereo_clip(track.raw()));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    close(hear(&mut control, &mut audio, frames), 0.25);

    let endpoint = control.device_control_handle(device.raw()).unwrap();
    let (sender, listed) = std::sync::mpsc::channel();
    let lane = std::thread::spawn(move || {
        let _ = sender.send(endpoint.parameters());
    });
    let (mut held, _) = listener.accept().unwrap();
    let mut reached = [0];
    held.read_exact(&mut reached).unwrap();

    // What the document worker does for other requests meanwhile.
    let (edited, _) = project.add_track("unrelated");
    control.replace(&edited).unwrap();
    close(hear(&mut control, &mut audio, frames), 0.25);
    assert!(!control.status().pending_plan);
    assert_eq!(control.device_applied_state(device.raw()).unwrap(), None);
    assert!(
        listed.try_recv().is_err(),
        "control work waited for the held parameter listing"
    );

    held.write_all(&[1]).unwrap();
    let infos = listed
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(infos[0].name, "Gain");
    lane.join().unwrap();
    let _ = std::fs::remove_file(&socket);
}
