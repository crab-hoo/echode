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
    assert_eq!(plugins.len(), 8);
    assert_eq!(plugins[0].id, "dev.echode.test-gain");
    assert_eq!(plugins[0].name, "Echode Test Gain");
    assert_eq!(plugins[0].vendor, "Echode");
    assert_eq!(plugins[0].version, "0.0.0");
    assert_eq!(plugins[0].features, ["audio-effect"]);
    assert_eq!(plugins[0].url, "https://echode.invalid");
    assert_eq!(plugins[1].id, "dev.echode.test-instrument");
    assert_eq!(plugins[1].features, ["instrument"]);
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
    assert_eq!(
        scanned
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        [
            "dev.echode.test-gain",
            "dev.echode.test-instrument",
            "dev.echode.test-mono",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-ports",
            "dev.echode.test-state",
            "dev.echode.test-zflip",
        ]
    );
    let found = echode_plugin_host::list_bundles(&root);
    let cache = root.join("scan.jsonl");
    let listed = echode_plugin_host::refresh_catalog(&host, &cache, &found).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        [
            "dev.echode.test-gain",
            "dev.echode.test-instrument",
            "dev.echode.test-mono",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-ports",
            "dev.echode.test-state",
            "dev.echode.test-zflip",
        ]
    );
    std::fs::write(macos.join("Plugin"), b"replaced after the cache").unwrap();
    let again = echode_plugin_host::refresh_catalog(&host, &cache, &found).unwrap();
    assert_eq!(
        again
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        [
            "dev.echode.test-gain",
            "dev.echode.test-instrument",
            "dev.echode.test-mono",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-ports",
            "dev.echode.test-state",
            "dev.echode.test-zflip",
        ]
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_folder_change_publishes_the_plugin_and_drops_it_when_the_bundle_goes_away() {
    let root = std::env::temp_dir().join(format!(
        "echode-live-watch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let cache = root.join("scan.jsonl");
    let live = echode_plugin_host::LiveCatalog::start(
        vec![root.clone()],
        host,
        cache,
        std::time::Duration::from_millis(200),
    )
    .expect("watch");
    let revision = live.wait_after(0, std::time::Duration::from_secs(5));
    assert!(revision >= 1, "the empty folder should publish");
    assert_eq!(listed(&live.json()), Vec::<(String, bool)>::new());
    // Let FSEvents arm before the first change, as the stream test does.
    std::thread::sleep(std::time::Duration::from_millis(200));

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
    let appeared = wait_for(&live, revision, |rows| {
        rows.iter()
            .any(|row| row.0 == "dev.echode.test-gain" && row.1)
    });
    assert!(
        appeared,
        "expected a fresh test plugin, got {:?}",
        listed(&live.json())
    );

    std::fs::remove_dir_all(&bundle).unwrap();
    let gone = wait_for(&live, live.revision(), |rows| {
        rows.iter().all(|row| row.0 != "dev.echode.test-gain")
    });
    assert!(
        gone,
        "the removed bundle stayed listed: {:?}",
        listed(&live.json())
    );
    drop(live);
    let _ = std::fs::remove_dir_all(&root);
}

fn listed(json: &[u8]) -> Vec<(String, bool)> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(json).expect("catalog json");
    rows.into_iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap_or_default().to_string(),
                row["fresh"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

fn wait_for(
    live: &echode_plugin_host::LiveCatalog,
    mut revision: u64,
    mut ready: impl FnMut(&[(String, bool)]) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        let rows = listed(&live.json());
        if ready(&rows) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        revision = live.wait_after(revision, std::time::Duration::from_millis(250));
    }
}

fn script(body: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "echode-scan-{}-{}.sh",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

const PLUGIN_LINE: &str = "{\"id\":\"dev.audit.scan\",\"name\":\"Scan\",\"vendor\":\"Echode\",\"version\":\"1\",\"features\":[],\"url\":\"\"}\n";

#[test]
fn audit_rea08_descendant_pipe_does_not_extend_deadline() {
    let json = script(&format!("#!/bin/sh\nprintf '%s' '{}'", PLUGIN_LINE));
    let started = std::time::Instant::now();
    let plugins = echode_plugin_host::scan_with_budget(
        &json,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect("json");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(plugins[0].id, "dev.audit.scan");
    let _ = std::fs::remove_file(json);

    let noisy = script(&format!(
        "#!/bin/sh\nprintf '%s' '{}'\ndd if=/dev/zero bs=1024 count=80 2>/dev/null | tr '\\0' x >&2\n",
        PLUGIN_LINE
    ));
    echode_plugin_host::scan_with_budget(
        &noisy,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect("stderr drained");
    let _ = std::fs::remove_file(noisy);

    let held = script(&format!(
        "#!/bin/sh\nprintf '%s' '{}'\n(sleep 30) &\nexit 0\n",
        PLUGIN_LINE
    ));
    let started = std::time::Instant::now();
    let error = echode_plugin_host::scan_with_budget(
        &held,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_millis(200),
    )
    .expect_err("grandchild holds the pipe");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_file(held);
    assert!(error.contains("timed out"), "{error}");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "waited {elapsed:?}"
    );

    let hang = script("#!/bin/sh\nsleep 30\n");
    let started = std::time::Instant::now();
    let error = echode_plugin_host::scan_with_budget(
        &hang,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_millis(200),
    )
    .expect_err("hang");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_file(hang);
    assert!(error.contains("timed out"), "{error}");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");

    let bad = script("#!/bin/sh\nprintf '\\377'\n");
    let error = echode_plugin_host::scan_with_budget(
        &bad,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect_err("utf-8");
    let _ = std::fs::remove_file(bad);
    assert!(error.contains("UTF-8"), "{error}");

    let failed = script("#!/bin/sh\necho boom >&2\nexit 1\n");
    let error = echode_plugin_host::scan_with_budget(
        &failed,
        std::path::Path::new("bundle.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect_err("exit");
    let _ = std::fs::remove_file(failed);
    assert!(error.contains("failed"), "{error}");
    assert!(error.contains("boom"), "{error}");
}
