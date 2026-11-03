use super::*;

#[test]
fn offline_middle_effect_keeps_stereo_input_and_neighboring_effects_under_rtsan() {
    let (mut project, track) = Project::new().add_track("stereo chain");
    // Three stages and this short stereo block exercise only this routing case.
    let devices: Vec<_> = (0..3).map(|_| Id::generate()).collect();
    for device in &devices {
        project = project
            .add_clap_device(track, *device, "dev.echode.test-gain")
            .unwrap();
    }
    let offline = project.set_device_offline(devices[1], true).unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
        state: None,
    };
    let frames = 64;
    let (mut control, mut audio) = playback_with_plugins(
        &offline,
        48_000,
        frames,
        &host_bin(),
        &[route.clone(), route.clone(), route],
    )
    .unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![LoadedClip {
            track: track.raw(),
            start_sample: 0,
            samples: vec![0.125; 4096].into(),
            right: Some(vec![-0.0625; 4096].into()),
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    control.replace(&offline).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    for (next, gain) in [(&offline, 4.0), (&project, 8.0), (&offline, 4.0)] {
        control.replace(next).unwrap();
        // Bound adoption of this fixture's queued plan replacements, not audio work.
        for _ in 0..8 {
            render_content(&mut audio, &mut left, &mut right);
            control.service();
            if !control.status().pending_plan {
                break;
            }
        }
        assert!(!control.status().pending_plan);
        assert!(
            left.iter()
                .all(|sample| (*sample - 0.125 * gain).abs() < 1.0e-6),
            "gain {gain}, left {}",
            left[0]
        );
        assert!(
            right
                .iter()
                .all(|sample| (*sample + 0.0625 * gain).abs() < 1.0e-6)
        );
    }
}

#[test]
fn contract_offline_releases_helpers_and_restores_recorded_state_under_rtsan() {
    let project = state_project();
    let device = project.tracks()[0].devices[0].id;
    let offline = project.set_device_offline(device, true).unwrap();
    let route = state_route(&plugin_dylib(), Some(vec![128]));
    // These block lengths exercise this fixture only; they are not engine limits.
    for block in [7, 64, 512] {
        let missing_host = host_bin().with_file_name("missing-offline-host");
        let (mut control, mut audio) = echode_engine::playback(
            &offline,
            48_000,
            block,
            echode_engine::PlaybackOptions::default()
                .plugins(&missing_host, std::slice::from_ref(&route)),
        )
        .expect("an offline device must never load its plugin");
        assert!(control.plugin_event_caps().is_empty());
        control.command(PlaybackCommand::Play).unwrap();
        let mut left = vec![0.0; block];
        let mut right = left.clone();
        render_content(&mut audio, &mut left, &mut right);
        assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
        control.set_routes(&host_bin(), std::slice::from_ref(&route));
        for _ in 0..3 {
            control.replace(&project).unwrap();
            render_content(&mut audio, &mut left, &mut right);
            control.service();
            assert_eq!(control.plugin_event_caps().len(), 1);
            assert!(
                left.iter()
                    .chain(&right)
                    .all(|sample| (*sample - 0.5).abs() < 1.0e-6)
            );
            control.replace(&offline).unwrap();
            render_content(&mut audio, &mut left, &mut right);
            control.service();
            assert!(control.plugin_event_caps().is_empty());
            assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
            assert!(control.take_preparation_error().is_none());
        }
    }
}
