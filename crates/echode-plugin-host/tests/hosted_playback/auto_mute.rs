//! A plugin's NaN auto-mutes only its track and never reaches the next plugin;
//! starting playback resets the track and lets it sound again (ADR 0282).

use super::*;

fn block(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_until(left, right, Instant::now() + Duration::from_secs(5));
}

#[test]
fn a_plugin_nan_auto_mutes_its_track_until_playback_starts() {
    // The rate, block and planes bound only this regression.
    let (rate, frames) = (48_000, 256);
    let (project, faulty) = Project::new().add_track("faulty instrument");
    let instrument = Id::generate();
    let project = project
        .add_clap_device(faulty, instrument, "dev.echode.test-instrument")
        .unwrap()
        .add_clap_device(faulty, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let (project, dry) = project.add_track("unaffected audio");
    let routes = [
        PluginRoute {
            clap_id: "dev.echode.test-instrument".into(),
            path: plugin_dylib(),
            instrument: true,
            fault: Some("nan".into()),
            state: None,
        },
        PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: None,
            state: None,
        },
    ];
    let (mut control, mut audio) =
        playback_with_plugins(&project, rate, frames, &host_bin(), &routes).unwrap();
    let dry_audio: Arc<[f32]> = vec![0.03125; rate as usize].into();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![LoadedClip {
            track: dry.raw(),
            start_sample: 0,
            samples: Arc::clone(&dry_audio),
            right: None,
            window: None,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let meter = control
        .meters()
        .into_iter()
        .find(|meter| meter.track == faulty.raw())
        .unwrap();
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    // Mono dry audio is panned at the centre of a stereo master.
    let dry_level = 0.03125 * std::f32::consts::FRAC_1_SQRT_2;
    block(&mut audio, &mut left, &mut right);
    assert!(meter.auto_mute().is_none());
    assert!(
        left.iter()
            .all(|sample| (*sample - (1.0 + dry_level)).abs() < 1.0e-6),
        "the instrument's 0.5 doubled by the effect: {}",
        left[0]
    );

    // From frame 10 of the second block the instrument writes NaN.
    block(&mut audio, &mut left, &mut right);
    let mute = meter.auto_mute().expect("the faulty track is auto-muted");
    assert_eq!(
        mute.device,
        Some(instrument.raw()),
        "the NaN reached the next plugin"
    );
    assert_eq!(mute.at, frames as i64 + 10);
    let failure = control
        .plugin_failure()
        .expect("export must refuse this block");
    assert_eq!(
        failure.reason,
        echode_engine::PluginBlockFailureReason::NonFinite
    );
    assert_eq!(failure.device, instrument.raw());
    assert!(
        failure
            .to_string()
            .ends_with("returned a NaN or an infinity")
    );
    assert!(!control.status().failed);
    assert_eq!(
        control.status().plugin_missed_blocks,
        0,
        "an answer is not a miss"
    );
    assert!(
        left[..10]
            .iter()
            .all(|sample| (*sample - (1.0 + dry_level)).abs() < 1.0e-6)
    );
    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
    // The rest fades like a miss and stays down while the plugin keeps failing.
    for _ in 0..8 {
        block(&mut audio, &mut left, &mut right);
    }
    assert!(
        left.iter()
            .all(|sample| (*sample - dry_level).abs() < 1.0e-6)
    );
    assert_eq!(
        meter.auto_mute(),
        Some(mute),
        "a later fault is not reported again"
    );

    // Starting playback releases the track and resets its plugins.
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        block(&mut audio, &mut left, &mut right);
        control.service();
        if left
            .iter()
            .all(|sample| (*sample - (1.0 + dry_level)).abs() < 1.0e-6)
        {
            break;
        }
        assert!(Instant::now() < deadline, "the track did not sound again");
    }
    assert!(meter.auto_mute().is_none());
    assert!(control.plugin_failure().is_none());
}
