//! Downstream note clocks must follow the audio entering that stage.
use super::*;

fn route(fault: &str) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some(fault.into()),
        state: None,
    }
}

fn score(folder: bool) -> Project {
    let (project, track) = Project::new().add_track("notes");
    let mut project = project;
    // These short notes and ticks bound this regression, not product polyphony.
    for (start, length) in [(0, 2), (4, 3), (9, 20)] {
        project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 127,
                    start: Tick::new(start * 147),
                    length: Tick::new(length * 147),
                },
            )
            .unwrap()
            .0;
    }
    let delay_track = if folder {
        let (next, child) = project.add_child(track, "delayed child").unwrap();
        project = next;
        child
    } else {
        track
    };
    project = project
        .add_clap_device(delay_track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap()
}

#[test]
fn downstream_notes_follow_input_audio_across_blocks_and_loops_under_rtsan() {
    // The delay exceeds the loop length at both test rates.
    for rate in [48_000, 96_000] {
        for folder in [false, true] {
            for block in [1, 7, 64, 512] {
                let project = score(folder);
                // Model order visits the parent before its child.
                let delayed = if folder {
                    [route("onset"), route("delay:37")]
                } else {
                    [route("delay:37"), route("onset")]
                };
                let dry = if folder {
                    [route("onset"), route("delay:0")]
                } else {
                    [route("delay:0"), route("onset")]
                };
                for looping in [false, true] {
                    for origin in [-3, 5] {
                        let render = |routes: &[PluginRoute]| {
                            let (mut control, mut audio) =
                                playback_with_plugins(&project, rate, block, &host_bin(), routes)
                                    .unwrap();
                            control.command(PlaybackCommand::Locate(origin)).unwrap();
                            if looping {
                                control
                                    .command(PlaybackCommand::SetLoop { start: 0, end: 30 })
                                    .unwrap();
                            }
                            control.command(PlaybackCommand::Play).unwrap();
                            let mut heard = Vec::new();
                            while heard.len() < 180 {
                                let count = block.min(180 - heard.len());
                                let mut left = vec![0.0; count];
                                let mut right = left.clone();
                                render_content(&mut audio, &mut left, &mut right);
                                assert!(!control.status().failed, "PDC note rendering failed");
                                heard.extend(left);
                            }
                            heard
                        };
                        let expected = render(&dry);
                        let actual = render(&delayed);
                        for (frame, actual) in actual.into_iter().enumerate() {
                            let expected = frame.checked_sub(37).map_or(0.0, |at| expected[at]);
                            assert!(
                                (actual - expected).abs() < 1.0e-6,
                                "PDC notes disagree with input audio: rate={rate}, folder={folder}, block={block}, looping={looping}, frame={frame}, actual={actual}, expected={expected}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn delayed_held_notes_survive_plan_adoption_and_stop_after_the_source_has_ended_under_rtsan() {
    let (project, track) = Project::new().add_track("short note");
    let mut project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::new(294),
            },
        )
        .unwrap()
        .0;
    for _ in 0..2 {
        project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
    }
    // Only this fixture's 10-sample note, 64-sample delay and 8-frame blocks.
    for stop in [false, true] {
        let (mut control, mut audio) = playback_with_plugins(
            &project,
            48_000,
            8,
            &host_bin(),
            &[route("delay:64"), route("count")],
        )
        .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = ([0.0; 8], [0.0; 8]);
        while audio.current_sample() < 72 {
            render_content(&mut audio, &mut left, &mut right);
        }
        assert_level(left[0], 1.0);
        if stop {
            control.command(PlaybackCommand::Stop).unwrap();
            render_content(&mut audio, &mut left, &mut right);
            control.command(PlaybackCommand::Locate(1_000)).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
        } else {
            control
                .replace(&project.add_track("unrelated edit").0)
                .unwrap();
        }
        render_content(&mut audio, &mut left, &mut right);
        assert_level(left[0], 17.0);
        render_content(&mut audio, &mut left, &mut right);
        assert_level(left[0], 17.0);
        assert!(!control.status().failed, "PDC note lifecycle failed");
    }
}

#[test]
fn a_missed_delayed_off_recovers_when_the_source_has_no_event_under_rtsan() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    let (project, track) = Project::new().add_track("delayed note-off");
    // Only this regression: off at sample 63, downstream off at 68 in block two.
    let mut project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::new(1853),
            },
        )
        .unwrap()
        .0;
    for _ in 0..3 {
        project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
    }
    let socket = PathBuf::from(format!("/tmp/echode-pdc-off-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket).unwrap();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        64,
        &host_bin(),
        &[
            route(&format!("onset-wait:{}", socket.display())),
            route("delay:5"),
            route("onset"),
        ],
    )
    .unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    render_content(&mut audio, &mut left, &mut right);
    assert!(left.iter().any(|sample| sample.abs() > 0.1));
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    let mut receipt = [0];
    peer.read_exact(&mut receipt).unwrap();
    for _ in 3..=5 {
        audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    }
    peer.write_all(&[1]).unwrap();
    peer.read_exact(&mut receipt).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let misses = control.status().plugin_missed_blocks;
        audio.render_live_gated_until(&mut left, &mut right, deadline, true);
        if control.status().plugin_missed_blocks == misses {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "delayed-off helper did not recover"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!control.status().failed);
    assert!(
        left.iter().chain(&right).all(|sample| *sample == 0.0),
        "a delayed note-off lost without a source event left a stuck voice"
    );
    std::fs::remove_file(socket).unwrap();
}
