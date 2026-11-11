//! An app started from the Finder begins with launchd's soft limit of 256
//! descriptors (`launchctl limit maxfiles`). The parent holds four for each
//! helper, so hosting must raise the soft limit before helpers run out of
//! room (ADR 0237). This file is its own test binary because it lowers the
//! limit for the whole process.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use echode_engine::{PlaybackCommand, PlaybackOptions, PluginRoute};
use echode_model::{Id, Project};

/// Launchd's default soft limit for a GUI app.
const FINDER_SOFT_LIMIT: libc::rlim_t = 256;
/// Four descriptors each puts this past the Finder limit. It bounds this test,
/// not the number of plugins.
const TRACKS: usize = 80;

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn plugin_dylib() -> PathBuf {
    static ARTIFACT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ARTIFACT.get_or_init(build_plugin_dylib).clone()
}

fn build_plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let sibling = host_bin()
        .parent()
        .unwrap()
        .join("libechode_test_plugin.dylib");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../echode-test-plugin/src");
    let current = |path: &Path| {
        let Ok(built) = path.metadata().and_then(|meta| meta.modified()) else {
            return false;
        };
        let Ok(edited) = newest_source(&source) else {
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

fn descriptor_limit() -> libc::rlimit {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit for the call's duration.
    let read = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    assert_eq!(read, 0);
    limit
}

#[test]
fn contract_many_helpers_start_from_the_finder_descriptor_limit() {
    let mut limit = descriptor_limit();
    assert!(
        limit.rlim_max > FINDER_SOFT_LIMIT,
        "hard limit too low to test"
    );
    limit.rlim_cur = FINDER_SOFT_LIMIT;
    // SAFETY: `limit` is a valid rlimit; lowering the soft limit is always allowed.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);

    let mut project = Project::new();
    for index in 0..TRACKS {
        let (next, track) = project.add_track(format!("t{index}"));
        project = next
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
    }
    let routes = [PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: None,
        state: None,
    }];
    let host = host_bin();
    let (mut control, mut audio) = echode_engine::playback(
        &project,
        48_000,
        256,
        PlaybackOptions::default().plugins(&host, &routes),
    )
    .expect("every helper starts");
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    audio.render_until(
        &mut left,
        &mut right,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
    );
    assert!(!control.status().failed);
    assert_eq!(control.status().plugin_missed_blocks, 0);
    assert!(descriptor_limit().rlim_cur > FINDER_SOFT_LIMIT);
}

fn newest_source(dir: &Path) -> std::io::Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}
