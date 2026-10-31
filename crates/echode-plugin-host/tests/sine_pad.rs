//! Bundled Sine Pad uses the same model and isolated CLAP graph as other instruments.
use echode_engine::{PlaybackCommand, PlaybackOptions, PluginRoute, playback};
use echode_model::{Id, NoteDraft, Project, Tick};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn route() -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.sine-pad".into(),
        path: PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
            .parent()
            .unwrap()
            .join("plugins/EchodeSinePad.clap"),
        instrument: true,
        fault: None,
        state: None,
    }
}

fn score() -> (Project, Id, Id) {
    let (project, track) = Project::new().add_track("pad");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap();
    let device = Id::generate();
    (
        project
            .add_clap_device(track, device, "dev.echode.sine-pad")
            .unwrap(),
        track,
        device,
    )
}

#[test]
fn contract_sine_pad_is_an_explicit_isolated_instrument_under_rtsan() {
    let route = route();
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let scanned = echode_plugin_host::scan_out_of_process(&host, &route.path).unwrap();
    assert!(scanned.iter().any(|plugin| {
        plugin.id == route.clap_id
            && plugin
                .features
                .iter()
                .any(|feature| feature == "instrument")
    }));
    let (project, _track, device) = score();
    let (mut control, mut audio) = playback(
        &project,
        48_000,
        128,
        PlaybackOptions::default().plugins(&host, &[route]),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    // These buffers and deadlines cover this deterministic, device-free regression.
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(2),
    );
    assert!(left.iter().any(|sample| *sample != 0.0));
    assert_eq!(left, right);
    assert_eq!(control.status().plugin_missed_blocks, 0);
    let bypassed = project.set_device_bypassed(device, true).unwrap();
    control.replace(&bypassed).unwrap();
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(2),
    );
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    let removed = project.remove_device(device).unwrap();
    control.service();
    control.replace(&removed).unwrap();
    audio.render_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(2),
    );
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
}

fn playback_pair(
    project: &Project,
    rate: u32,
    frames: usize,
) -> Result<
    (echode_engine::PlaybackControl, echode_engine::PlaybackAudio),
    echode_engine::PlaybackError,
> {
    playback(
        project,
        rate,
        frames,
        PlaybackOptions::default().plugins(
            &PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host")),
            &[route()],
        ),
    )
}
fn render_content(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_until(left, right, Instant::now() + Duration::from_secs(2));
}
#[test]
fn pause_and_stop_hold_the_position_and_seek_chases_a_note() {
    let (project, _, _) = score();
    // Scratch lengths belong to this probe, never to a product track cap.
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().all(|sample| *sample == 0.0));
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().sample, 64);
    assert!(left.iter().any(|sample| *sample != 0.0));
    control.command(PlaybackCommand::Pause).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().sample, 64);
    assert!(!control.status().playing);
    assert!(left.iter().all(|sample| *sample == 0.0));
    control.command(PlaybackCommand::Locate(24000)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    assert_eq!(control.status().sample, 24064);
    control.command(PlaybackCommand::Stop).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().sample, 24064);
    assert!(!control.status().playing);
    assert!(left.iter().all(|sample| *sample == 0.0));
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert_eq!(control.status().sample, 24128);
    assert!(left.iter().any(|sample| *sample != 0.0));
}

#[test]
fn an_unchanged_live_voice_keeps_its_envelope_and_phase_across_an_edit() {
    let (project, _, _) = score();
    let pad = project.tracks()[0].id;
    let (changed, folder) = project.add_track("empty folder");
    // Reorder the pad through the model's folder operation. Playback keys keep
    // their identity even when their prepared track indices change.
    let changed = changed.set_parent(pad, Some(folder)).unwrap();
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    let (mut reference_control, mut reference) = playback_pair(&project, 48000, 64).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    reference_control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right, mut expected_left, mut expected_right) =
        ([0.0; 64], [0.0; 64], [0.0; 64], [0.0; 64]);
    for block in 0..1100 {
        if block == 20 || block == 800 {
            control.replace(&changed).unwrap();
        }
        render_content(&mut audio, &mut left, &mut right);
        render_content(&mut reference, &mut expected_left, &mut expected_right);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        control.service();
    }
}

#[test]
fn deleting_a_sounding_note_silences_it_in_the_next_plan() {
    let (project, _, _) = score();
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    control.replace(&Project::new()).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().all(|sample| *sample == 0.0));
    assert_eq!(control.status().sample, 128);
}

#[test]
fn sine_pad_grows_prepared_voices_and_keeps_phase_under_rtsan() {
    let (project, track, _) = score();
    let mut expanded = project.clone();
    // The 160 future notes exercise capacity growth beyond a typical fixed
    // polyphony; their count and these buffers bound only this regression.
    for offset in 0..160 {
        expanded = expanded
            .insert_note(
                track,
                NoteDraft {
                    pitch: 64,
                    velocity: 100,
                    start: Tick::quarters(4 + offset).unwrap(),
                    length: Tick::quarters(1).unwrap(),
                },
            )
            .unwrap()
            .0;
    }
    let (mut control, mut audio) = playback_pair(&project, 48_000, 64).unwrap();
    let (mut expected_control, mut expected) = playback_pair(&project, 48_000, 64).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    expected_control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right, mut wanted_left, mut wanted_right) =
        ([0.0; 64], [0.0; 64], [0.0; 64], [0.0; 64]);
    for block in 0..32 {
        if block == 4 {
            control.replace(&expanded).unwrap();
        }
        render_content(&mut audio, &mut left, &mut right);
        render_content(&mut expected, &mut wanted_left, &mut wanted_right);
        assert_eq!(left, wanted_left, "phase changed at block {block}");
        assert_eq!(right, wanted_right);
        assert_eq!(control.status().plugin_missed_blocks, 0);
        control.service();
    }
    // A seek after plan adoption must chase current notes again.
    control.command(PlaybackCommand::Locate(1_000)).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
}

#[test]
fn sine_pad_state_and_declared_tail_use_the_standard_helper_protocol_under_rtsan() {
    use echode_sandbox::{PluginHost, SessionLayout, SpawnOptions};
    let route = route();
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    for rate in [48_000, 96_000] {
        let mut plugin = PluginHost::spawn(SpawnOptions {
            host_bin: &host,
            plugin: Some(&route.path),
            plugin_id: Some(&route.clap_id),
            fault: None,
            workgroup: None,
            fan_in: None,
            sample_rate: rate,
            layout: SessionLayout::stereo(),
        })
        .unwrap();
        assert_eq!(plugin.tail_samples(), rate * 400 / 1000);
        let state = plugin.save_state().unwrap();
        plugin.restore_state(&state).unwrap();
        // The first process on a fresh helper also exercises cold thread state.
        let mut output = [0.0; 64];
        plugin
            .process_events(
                &[],
                &mut [&mut output],
                &[echode_engine::Event::note_on(0, 0, 1, 69, 1.0)],
                &mut [],
                Duration::from_secs(2),
            )
            .unwrap_or_else(|error| panic!("{error}: {}", plugin.stderr_log()));
        assert!(output.iter().any(|sample| *sample != 0.0));
        assert!(plugin.restore_state(b"invalid").is_err());
    }
}
