//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
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
