//! Product playback through the test gain plugin. The helper is a separate process.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use echode_engine::{PlaybackCommand, PluginRoute, playback_pair, playback_with_plugins};
use echode_model::{Id, NoteDraft, Project, Tick};

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let sibling = host_bin()
        .parent()
        .unwrap()
        .join("libechode_test_plugin.dylib");
    if sibling.exists() {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if sibling.exists() {
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
    };
    let replaced = render(&with_gain, Some(instrument), 64, 2);
    assert!(replaced.iter().all(|sample| *sample == 0.0));
}

#[test]
fn plugin_playback_does_not_depend_on_the_block_size() {
    let (project, track) = score();
    let project = project
        .add_clap_device(track, Id::from_raw(91), "dev.echode.test-gain")
        .unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
    };
    // One frame is shorter than a helper round trip, so that block is defined
    // to be silent. These lengths all finish inside the block.
    let reference = render(&project, Some(route.clone()), 64, 1);
    for (frames, blocks) in [(8usize, 8usize), (16, 4), (32, 2)] {
        let heard = render(&project, Some(route.clone()), frames, blocks);
        assert_eq!(heard.len(), reference.len());
        assert_eq!(heard, reference);
    }
}
