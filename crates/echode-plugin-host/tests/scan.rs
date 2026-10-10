//! The helper reads CLAP descriptors. The test plugin is not instantiated.

use echode_plugin_host::{
    cache_entry, cached_plugins, load_scan_cache, scan_descriptors, scan_report, store_scan_cache,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

fn plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let sibling = host.parent().unwrap().join("libechode_test_plugin.dylib");
    if sibling.exists() {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if sibling.exists() {
        return sibling;
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin");
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg("build")
        .arg("-p")
        .arg("echode-test-plugin")
        .arg("--target-dir")
        .arg(&target);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command.status().expect("spawn cargo for the test plugin");
    assert!(status.success(), "failed to build echode-test-plugin");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    target.join(profile).join("libechode_test_plugin.dylib")
}

#[test]
fn scanning_reads_the_factory_descriptor() {
    let plugins = scan_descriptors(&plugin_dylib()).expect("scan");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "dev.echode.test-gain");
    assert_eq!(plugins[0].name, "Echode Test Gain");
    assert_eq!(plugins[0].vendor, "Echode");
    assert_eq!(plugins[0].version, "0.0.0");
    assert_eq!(plugins[0].features, ["audio-effect"]);
    assert_eq!(plugins[0].url, "https://echode.invalid");
    let parsed: Vec<echode_plugin_host::ScannedPlugin> = scan_report(&plugins)
        .lines()
        .map(|line| serde_json::from_str(line).expect("report line"))
        .collect();
    assert_eq!(parsed, plugins);
}

#[test]
fn the_scan_cache_round_trips_and_follows_the_bundle_revision() {
    let root = std::env::temp_dir().join(format!("echode-scan-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundle = root.join("Synth.clap");
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(macos.join("Plugin"), b"v1").unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
    )
    .unwrap();
    let plugins = vec![echode_plugin_host::ScannedPlugin {
        id: "dev.echode.synth".into(),
        name: "Synth".into(),
        vendor: "Echode".into(),
        version: "1.2.3".into(),
        features: vec!["instrument".into()],
        url: "https://echode.invalid".into(),
    }];
    let entry = cache_entry(&bundle, plugins.clone());
    let cache = root.join("scan.jsonl");
    store_scan_cache(&cache, &[entry]).unwrap();
    let loaded = load_scan_cache(&cache).unwrap();
    assert_eq!(loaded[0].plugins, plugins);
    assert_eq!(loaded[0].bundle_version, "1");
    let revision = echode_plugin_host::list_bundles(&root)[0].revision.clone();
    assert_eq!(
        cached_plugins(&loaded, &bundle, &revision).unwrap(),
        plugins.as_slice()
    );

    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>2</string>
</dict></plist>"#,
    )
    .unwrap();
    let next = echode_plugin_host::list_bundles(&root)[0].revision.clone();
    assert!(cached_plugins(&loaded, &bundle, &next).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_helper_scans_a_bundle_and_a_second_refresh_uses_the_cache() {
    let root = std::env::temp_dir().join(format!(
        "echode-scan-proc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let bundle = root.join("Gain.clap");
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::copy(plugin_dylib(), macos.join("Plugin")).unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
    )
    .unwrap();
    let broken = root.join("Broken.clap");
    let broken_macos = broken.join("Contents/MacOS");
    std::fs::create_dir_all(&broken_macos).unwrap();
    std::fs::write(broken_macos.join("Plugin"), b"not a plugin").unwrap();
    std::fs::write(
        broken.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
    )
    .unwrap();
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let scanned = echode_plugin_host::scan_out_of_process(&host, &bundle).unwrap();
    assert_eq!(scanned[0].id, "dev.echode.test-gain");
    let found = echode_plugin_host::list_bundles(&root);
    let cache = root.join("scan.jsonl");
    let listed = echode_plugin_host::refresh_catalog(&host, &cache, &found).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "dev.echode.test-gain");
    std::fs::write(macos.join("Plugin"), b"replaced after the cache").unwrap();
    let again = echode_plugin_host::refresh_catalog(&host, &cache, &found).unwrap();
    assert_eq!(again[0].id, "dev.echode.test-gain");
    let _ = std::fs::remove_dir_all(&root);
}
