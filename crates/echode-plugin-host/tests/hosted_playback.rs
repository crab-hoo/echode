//! Product playback through the test gain plugin. The helper is a separate process.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use echode_sandbox::{PluginHost, SessionLayout, SpawnOptions};

use echode_engine::{
    LoadedClip, PlaybackCommand, PluginRoute, playback_pair, playback_with_plugins, tick_to_sample,
};
use echode_model::{AudioRef, Id, NoteDraft, NoteGeometry, Project, Tick};

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let sibling = host_bin()
        .parent()
        .unwrap()
        .join("libechode_test_plugin.dylib");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../echode-test-plugin/src/lib.rs");
    let current = |path: &Path| {
        let Ok(built) = path.metadata().and_then(|meta| meta.modified()) else {
            return false;
        };
        let Ok(edited) = source.metadata().and_then(|meta| meta.modified()) else {
            return false;
        };
        built >= edited
    };
    if current(&sibling) {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if current(&sibling) {
        return sibling;
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin");
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["build", "-p", "echode-test-plugin", "--target-dir"])
        .arg(&target)
        .status()
        .expect("build test plugin");
    assert!(status.success());
    target.join("debug").join("libechode_test_plugin.dylib")
}

fn score() -> (Project, Id) {
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0;
    (project, track)
}

fn render(project: &Project, route: Option<PluginRoute>, frames: usize, blocks: usize) -> Vec<f32> {
    let (mut control, mut audio) = if let Some(route) = route {
        playback_with_plugins(project, 48_000, frames, &host_bin(), &[route]).unwrap()
    } else {
        playback_pair(project, 48_000, frames).unwrap()
    };
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    let mut heard = Vec::new();
    for _ in 0..blocks {
        audio.render(&mut left, &mut right);
        heard.extend_from_slice(&left);
        control.service();
    }
    heard
}

#[test]
#[ignore = "uses realtime block deadlines; make spike-isolation runs the release test"]
fn a_gain_plugin_doubles_the_builtin_synth_and_an_instrument_replaces_it() {
    let (project, track) = score();
    let with_gain = project
        .add_clap_device(track, Id::from_raw(90), "dev.echode.test-gain")
        .unwrap();
    let path = plugin_dylib();
    let effect = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: false,
        fault: None,
    };
    let dry = render(&project, None, 64, 4);
    let wet = render(&with_gain, Some(effect), 64, 4);
    assert!(dry.iter().any(|sample| *sample != 0.0));
    assert_eq!(dry.len(), wet.len());
    for (dry, wet) in dry.iter().zip(&wet) {
        assert!((wet - dry * 2.0).abs() < 1.0e-5, "{wet} != {dry} * 2");
    }
    let instrument = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path,
        instrument: true,
        fault: None,
    };
    let replaced = render(&with_gain, Some(instrument), 64, 2);
    assert!(replaced.iter().all(|sample| *sample == 0.0));
}

#[test]
#[ignore = "uses realtime block deadlines; make spike-isolation runs the release test"]
fn plugin_playback_does_not_depend_on_the_block_size() {
    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-gain")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
    };
    // This is a timing-sensitive product integration check. Ordinary correctness
    // tests below use a hang guard instead of the realtime block deadline.
    let reference = render(&project, Some(route.clone()), 64, 1);
    for (frames, blocks) in [(8usize, 8usize), (16, 4), (32, 2)] {
        let heard = render(&project, Some(route.clone()), frames, blocks);
        assert_eq!(heard.len(), reference.len());
        assert_eq!(heard, reference);
    }
}

#[test]
fn plugin_output_is_independent_of_partition_without_a_realtime_deadline() {
    let (project, _) = score();
    let source = render(&project, None, 64, 1);
    let expected: Vec<_> = source.iter().map(|sample| sample * 2.0).collect();
    assert!(expected.iter().any(|sample| *sample != 0.0));
    let plugin = plugin_dylib();
    for frames in [1, 7, 8, 14, 28, 64] {
        let mut host = PluginHost::spawn(SpawnOptions {
            host_bin: &host_bin(),
            plugin: Some(&plugin),
            fault: None,
            workgroup: None,
            // The capacity covers this 64-sample fixture only, not a product limit.
            layout: SessionLayout {
                max_frames: 64,
                input_channels: 2,
                output_channels: 2,
                input_events: 1,
                output_events: 1,
            },
            fan_in: None,
            sample_rate: 48_000,
            plugin_id: Some("dev.echode.test-gain"),
        })
        .unwrap();
        let mut heard = Vec::new();
        for chunk in source.chunks(frames) {
            let mut left = vec![0.0; chunk.len()];
            let mut right = vec![0.0; chunk.len()];
            // This bounds a hung helper, not its realtime processing performance.
            host.process(
                &[chunk, chunk],
                &mut [&mut left, &mut right],
                Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(left, right);
            heard.extend(left);
        }
        assert_eq!(heard, expected, "partition {frames}");
    }
}

#[test]
fn audit_aud07_one_deadline_collects_a_healthy_track_beside_hangs() {
    let note = NoteDraft {
        pitch: 64,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::quarters(2).unwrap(),
    };
    let (project, live) = Project::new().add_track("live");
    let mut project = project.insert_note(live, note).unwrap().0;
    let mut hang_ids = Vec::new();
    for name in ["hang-a", "hang-b", "hang-c"] {
        let (next, id) = project.add_track(name);
        project = next.insert_note(id, note).unwrap().0;
        hang_ids.push(id);
    }
    let mut device = 90u128;
    project = project
        .add_clap_device(live, Id::from_raw(device), "dev.echode.test-gain")
        .unwrap();
    for id in hang_ids {
        device += 1;
        project = project
            .add_clap_device(id, Id::from_raw(device), "dev.echode.test-gain")
            .unwrap();
    }
    let path = plugin_dylib();
    let mut routes = vec![PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: false,
        fault: None,
    }];
    for _ in 0..3 {
        routes.push(PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: path.clone(),
            instrument: false,
            fault: Some("hang".into()),
        });
    }
    let (dry_project, dry_track) = Project::new().add_track("live");
    let dry_project = dry_project.insert_note(dry_track, note).unwrap().0;
    let dry = render(&dry_project, None, 2048, 1);
    let expected: Vec<_> = dry.iter().map(|sample| sample * 2.0).collect();
    assert!(expected.iter().any(|sample| *sample != 0.0));

    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 2048, &host_bin(), &routes).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 2048];
    let mut right = vec![0.0; 2048];
    let started = std::time::Instant::now();
    audio.render(&mut left, &mut right);
    let elapsed = started.elapsed();
    // Four helpers that each waited out one 42.7 ms block took about 176 ms.
    // Sharing one deadline stays near a single block. 150 ms separates those
    // outcomes; it is not a performance budget.
    assert!(
        elapsed < Duration::from_millis(150),
        "four helpers stacked deadlines: {elapsed:?}"
    );
    for (heard, expected) in left.iter().zip(&expected) {
        assert!(
            (heard - expected).abs() < 1.0e-4,
            "{heard} != {expected}; a hang was mixed in or the live track was dropped"
        );
    }
    assert!(!control.status().failed);
}

fn count_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: true,
        fault: Some("count".into()),
    }
}

fn level(events: f32) -> f32 {
    events * std::f32::consts::FRAC_1_SQRT_2
}

fn assert_level(heard: f32, events: f32) {
    let expected = level(events);
    assert!(
        (heard - expected).abs() < 1.0e-3,
        "{heard} != {expected} ({events} counted events)"
    );
}

#[test]
fn audit_aud05_notes_added_after_the_plugin_all_reach_it_and_its_state_stays() {
    let path = plugin_dylib();
    let route = count_route(&path);
    let frames = 256usize;
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(90), "dev.echode.test-gain")
        .unwrap();
    let short = Tick::new(941);
    let off = tick_to_sample(short, project.tempo(), 48_000).unwrap();
    assert!(
        off > 0 && off < frames as i64,
        "note end {off} leaves the block"
    );
    let mut chord = project.clone();
    for pitch in [60u8, 64, 67] {
        chord = chord
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: short,
                },
            )
            .unwrap()
            .0;
    }
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.replace(&chord).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    audio.render(&mut left, &mut right);
    // Three note-ons and three note-offs. The counter keeps both totals.
    assert_level(left[0], 3.0 + 3.0 * 16.0);
    assert!(!control.status().failed);

    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-gain")
        .unwrap();
    let long = Tick::quarters(2).unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap()
        .0;
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, frames, &host_bin(), &[route]).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    audio.render(&mut left, &mut right);
    assert_level(left[0], 1.0);
    let mut grown = project;
    for pitch in [64u8, 67] {
        grown = grown
            .insert_note(
                track,
                NoteDraft {
                    pitch,
                    velocity: 100,
                    start: Tick::new(0),
                    length: long,
                },
            )
            .unwrap()
            .0;
    }
    control.replace(&grown).unwrap();
    audio.render(&mut left, &mut right);
    // The first note-on is still inside the plugin. Only the two new ones are sent.
    assert_level(left[0], 3.0);
    assert!(!control.status().failed);
}

fn key_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: true,
        fault: Some("key".into()),
    }
}

fn hold_note(pitch: u8) -> (Project, Id, Id) {
    let (project, track) = Project::new().add_track("pad");
    let project = project
        .add_clap_device(track, Id::from_raw(92), "dev.echode.test-gain")
        .unwrap();
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap();
    (project, track, note)
}

fn first_sample(audio: &mut echode_engine::PlaybackAudio, frames: usize) -> f32 {
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    audio.render(&mut left, &mut right);
    left[0]
}

#[test]
fn audit_aud06_delete_pitch_locate_loop_and_stop_release_the_plugin() {
    let path = plugin_dylib();
    let frames = 256usize;
    let long = Tick::quarters(2).unwrap();

    let (project, track, note) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    let deleted = project.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    // The note-on is still counted. The delete adds one note-off.
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, _, _) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    control.command(PlaybackCommand::Locate(96_000)).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    control.command(PlaybackCommand::Stop).unwrap();
    let silent = first_sample(&mut audio, frames);
    assert!(silent.abs() < 1.0e-6, "stop wrote {silent}");
    let deleted = project.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    // The stop callback delivered the note-off. The replacement plan has no note to send.
    assert_level(first_sample(&mut audio, frames), 1.0 + 16.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let clip = project
        .tracks()
        .iter()
        .find(|found| found.id == track)
        .and_then(|found| found.clips.first())
        .unwrap()
        .id;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    let retuned = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 64,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap();
    control.replace(&retuned).unwrap();
    // A pitch edit keeps the voice. No note-off and no second note-on.
    assert_level(first_sample(&mut audio, frames), 1.0);
    assert!(!control.status().failed);

    let (project, track, note) = hold_note(60);
    let clip = project
        .tracks()
        .iter()
        .find(|found| found.id == track)
        .and_then(|found| found.clips.first())
        .unwrap()
        .id;
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&key_route(&path)),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 0.0);
    let retuned = project
        .edit_note(
            track,
            clip,
            note,
            NoteGeometry {
                pitch: 64,
                start: Tick::new(0),
                length: long,
            },
        )
        .unwrap();
    control.replace(&retuned).unwrap();
    assert_level(first_sample(&mut audio, frames), 0.0);
    let deleted = retuned.delete_note(track, note).unwrap();
    control.replace(&deleted).unwrap();
    // The note-off uses the key the note started on, not the edited pitch.
    assert_level(first_sample(&mut audio, frames), 60.0);
    assert!(!control.status().failed);

    let (project, _, _) = hold_note(60);
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&count_route(&path)),
    )
    .unwrap();
    control
        .command(PlaybackCommand::SetLoop {
            start: 0,
            end: frames as i64,
        })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_level(first_sample(&mut audio, frames), 1.0);
    // The wrap releases the sounding note, then strikes the one at the loop head.
    assert_level(first_sample(&mut audio, frames), 2.0 + 16.0);
    assert!(!control.status().failed);
}

fn effect_route(path: &Path) -> PluginRoute {
    PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.to_path_buf(),
        instrument: false,
        fault: None,
    }
}

fn constant_clips(track: u128, value: f32, frames: usize) -> echode_engine::ClipLoader {
    let samples: Arc<[f32]> = vec![value; frames].into();
    Arc::new(move |_project, _rate| {
        Ok(vec![LoadedClip {
            track,
            start_sample: 0,
            samples: Arc::clone(&samples),
            fade_in: 0,
            fade_out: 0,
        }])
    })
}

fn play_left(
    project: &Project,
    route: Option<PluginRoute>,
    track: u128,
    clip: Option<f32>,
    frames: usize,
) -> Vec<f32> {
    let (mut control, mut audio) = if let Some(route) = route {
        playback_with_plugins(project, 48_000, frames, &host_bin(), &[route]).unwrap()
    } else {
        playback_pair(project, 48_000, frames).unwrap()
    };
    if let Some(value) = clip {
        control.set_clip_loader(constant_clips(track, value, frames));
        control.replace(project).unwrap();
    }
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    audio.render(&mut left, &mut right);
    assert!(!control.status().failed);
    left
}

fn assert_close(heard: &[f32], expected: &[f32]) {
    assert_eq!(heard.len(), expected.len());
    for (heard, expected) in heard.iter().zip(expected) {
        assert!((heard - expected).abs() < 1.0e-5, "{heard} != {expected}");
    }
}

#[test]
fn audit_aud08_clips_and_notes_reach_the_track_effect_before_the_fader() {
    let path = plugin_dylib();
    let frames = 256usize;
    let gain = effect_route(&path);
    let pan = std::f32::consts::FRAC_1_SQRT_2;
    // The clip's first and last samples are the edge fade. The body is the constant.
    let body = frames / 2;

    let (project, track) = Project::new().add_track("audio");
    let project = project
        .add_clap_device(track, Id::from_raw(93), "dev.echode.test-gain")
        .unwrap();
    let audio = AudioRef::try_new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        48_000,
        1,
        frames as u64,
    )
    .unwrap();
    let project = project
        .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
        .unwrap();
    let dry_audio = play_left(&project, None, track.raw(), Some(0.25), frames);
    let wet_audio = play_left(
        &project,
        Some(gain.clone()),
        track.raw(),
        Some(0.25),
        frames,
    );
    // 0.25 through center pan, then the same clip after a gain of 2.
    assert!((dry_audio[body] - 0.25 * pan).abs() < 1.0e-5);
    assert!((wet_audio[body] - 0.35355338).abs() < 1.0e-5);
    let doubled: Vec<_> = dry_audio.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_audio, &doubled);

    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(94), "dev.echode.test-gain")
        .unwrap();
    let dry_notes = play_left(&project, None, track.raw(), None, frames);
    let wet_notes = play_left(&project, Some(gain.clone()), track.raw(), None, frames);
    let doubled: Vec<_> = dry_notes.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_notes, &doubled);
    assert!(dry_notes.iter().any(|sample| *sample != 0.0));

    // A note and a loaded clip can sound in one block. The model still rejects
    // two placements that cover the same time, so this clip is not a second
    // placement; it is the audio the control thread hands to playback.
    let dry_both = play_left(&project, None, track.raw(), Some(0.25), frames);
    let wet_both = play_left(
        &project,
        Some(gain.clone()),
        track.raw(),
        Some(0.25),
        frames,
    );
    let doubled: Vec<_> = dry_both.iter().map(|sample| sample * 2.0).collect();
    assert_close(&wet_both, &doubled);
    assert!((dry_both[body] - (dry_notes[body] + 0.25 * pan)).abs() < 1.0e-4);

    let (project, track) = Project::new().add_track("instrument");
    let project = project
        .add_clap_device(track, Id::from_raw(95), "dev.echode.test-gain")
        .unwrap();
    let instrument = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: path.clone(),
        instrument: true,
        fault: None,
    };
    let replaced = play_left(&project, Some(instrument), track.raw(), Some(0.25), frames);
    // The instrument hears silence, not the clip. The clip is added to its output.
    assert_close(&replaced, &dry_audio);

    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(96), "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(track, Id::from_raw(97), "dev.echode.test-gain")
        .unwrap();
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        &[count_route(&path), gain],
    )
    .unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.25, frames));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    audio.render(&mut left, &mut right);
    assert!(!control.status().failed);
    // Count writes 1 and ignores its input. The clip is mixed onto that output,
    // then the gain effect doubles the sum.
    let expected = (1.0 + 0.25) * 2.0 * pan;
    assert!(
        (left[body] - expected).abs() < 1.0e-4,
        "{} != {expected}",
        left[body]
    );
}
