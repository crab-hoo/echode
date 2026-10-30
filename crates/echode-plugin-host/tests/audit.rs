//! Open items from docs/reviews/2026-10-08-audit.md and
//! docs/reviews/2026-10-30-structure-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use echode_plugin_host::{
    FolderWatch, PluginCatalog, ScannedPlugin, list_bundles, list_bundles_including_clap_path,
};

fn scratch(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "echode-audit-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn make_bundle(path: &Path, executable: &[u8]) {
    std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    std::fs::write(path.join("Contents/MacOS/Plugin"), executable).unwrap();
}

#[test]
fn audit_plg1_a_bundle_in_a_vendor_subfolder_is_listed() {
    let root = scratch("plg1");
    let bundle = root.join("Vendor").join("Deep.clap");
    make_bundle(&bundle, b"v1");
    let listed = list_bundles(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        listed.iter().any(|found| found.path == bundle),
        "{listed:?}"
    );
}

#[test]
fn audit_plg1_clap_path_is_searched() {
    let root = scratch("plg1-path");
    let bundle = root.join("FromPath.clap");
    std::fs::create_dir_all(&bundle).unwrap();
    let listed = list_bundles_including_clap_path(&[], Some(root.to_str().unwrap()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        listed.iter().any(|found| found.path == bundle),
        "{listed:?}"
    );
}

#[test]
fn audit_plg2_rewriting_the_executable_inside_a_bundle_triggers_a_rescan() {
    let root = scratch("plg2");
    let bundle = root.join("Synth.clap");
    make_bundle(&bundle, b"v1");
    let mut catalog = PluginCatalog::new(Duration::ZERO);
    catalog.observe(Instant::now(), &list_bundles(&root));
    catalog.note_scanned(
        &bundle,
        vec![ScannedPlugin {
            id: "dev.echode.synth".into(),
            name: "Synth".into(),
            vendor: "Echode".into(),
            version: String::new(),
            features: Vec::new(),
            url: String::new(),
        }],
    );
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(bundle.join("Contents/MacOS/Plugin"), b"v2, a newer build").unwrap();
    catalog.observe(Instant::now(), &list_bundles(&root));
    let due = catalog.due(Instant::now());
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(due, vec![bundle]);
}

#[test]
fn audit_plg9_a_watch_can_be_dropped_right_after_it_starts() {
    let root = scratch("plg9");
    for _ in 0..100 {
        let (done, finished) = mpsc::channel();
        let watched = root.clone();
        std::thread::spawn(move || {
            drop(FolderWatch::start(&watched).unwrap());
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(3)).is_ok(),
            "drop did not return within 3 s"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The test plugin and helper from this checkout, shared with `hosted_playback`.
fn helper_fixture() -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = root.join("target/test-plugin");
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["build", "-q", "-p", "echode-test-plugin", "--target-dir"])
        .arg(&target)
        .status()
        .expect("build test plugin");
    assert!(status.success());
    (
        PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host")),
        target.join("debug").join("libechode_test_plugin.dylib"),
    )
}

/// docs/reviews/2026-10-30-structure-audit.md SPG-01. A timed-out save
/// used to replace the next restore's bytes. An in-flight call must now keep
/// its own input and reply; a rejected restore leaves the plugin unchanged.
#[test]
fn audit_spg01_a_timed_out_save_does_not_replace_the_next_restore_bytes() {
    use echode_sandbox::{PluginHost, PluginNotice, SessionLayout, SpawnOptions};

    let (host_bin, plugin) = helper_fixture();
    let mut host = PluginHost::spawn(SpawnOptions {
        host_bin: &host_bin,
        plugin: Some(plugin.as_path()),
        fault: Some("slow-save"),
        workgroup: None,
        layout: SessionLayout {
            max_frames: 64,
            input_channels: 2,
            output_channels: 2,
            input_events: 4,
            output_events: 1,
        },
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some("dev.echode.test-state"),
    })
    .expect("state plugin");
    // This regression's 64-frame buffer is not a product capacity limit.
    let input = vec![0.0; 64];
    let mut left = vec![0.0; 64];
    let mut right = vec![0.0; 64];
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("initial");
    assert!((left[0] - 0.25).abs() < 1.0e-4, "{}", left[0]);

    // The first save sleeps 3 s, past both 2 s waits.
    assert!(host.save_state().is_err(), "the slow save must time out");
    let restored = host.restore_state(&[128]);
    // Allow the earlier main-thread call to finish before checking its effects.
    std::thread::sleep(Duration::from_secs(4));
    let mut loaded = false;
    while let Some(notice) = host.poll_notice().expect("notice") {
        loaded |= matches!(notice, PluginNotice::Dirty);
    }
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .expect("after restore");
    if loaded {
        assert!(
            (left[0] - 0.5).abs() < 1.0e-4,
            "the helper loaded bytes other than the requested [128]: level {}, restore {restored:?}",
            left[0]
        );
    } else {
        assert!(
            restored.is_err(),
            "restore reported success without loading"
        );
        assert!(
            (left[0] - 0.25).abs() < 1.0e-4,
            "rejected restore changed state"
        );
    }
    // The old save response must not reject this later restore, nor satisfy
    // this new save. Both calls use the product control-only endpoint.
    let endpoint = host.control_handle();
    endpoint
        .restore_state(&[128])
        .expect("restore after completion");
    assert_eq!(endpoint.capture_state().unwrap(), [128]);
    assert_eq!(endpoint.applied_state().unwrap(), [128]);
    host.process(
        &[&input, &input],
        &mut [&mut left, &mut right],
        Duration::from_secs(2),
    )
    .unwrap();
    assert!((left[0] - 0.5).abs() < 1.0e-4, "restored level {}", left[0]);
}
