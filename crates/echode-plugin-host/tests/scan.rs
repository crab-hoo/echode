//! The helper reads CLAP descriptors. The test plugin is not instantiated.

use echode_plugin_host::{
    BundleFound, BundleRevision, CatalogTiming, ScanError, cache_entry, cached_plugins,
    load_scan_cache, scan_descriptors, scan_report, store_scan_cache,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

fn plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let sibling = host.parent().unwrap().join("libechode_test_plugin.dylib");
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
    assert_eq!(plugins.len(), 10);
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
            "dev.echode.test-monogain",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-params",
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
            "dev.echode.test-monogain",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-params",
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
            "dev.echode.test-monogain",
            "dev.echode.test-noports",
            "dev.echode.test-order",
            "dev.echode.test-params",
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
        CatalogTiming {
            quiet: std::time::Duration::from_millis(200),
            ..CatalogTiming::PRODUCT
        },
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

const PLUGIN_LINE: &str = "{\"id\":\"dev.audit.scan\",\"name\":\"Scan\",\"vendor\":\"Echode\",\"version\":\"1\",\"features\":[],\"url\":\"\"}\n";

#[test]
fn failed_scans_report_exit_codes_and_signals_without_stderr() {
    let host = Path::new(env!("CARGO_BIN_EXE_echode-scan-test-helper"));
    for (mode, status) in [
        ("exit.clap", "exit status: 23"),
        ("signal.clap", "signal: 15"),
    ] {
        let error = echode_plugin_host::scan_out_of_process(host, Path::new(mode))
            .expect_err("scan must fail");
        assert!(matches!(error, ScanError::Rejected(_)), "{error:?}");
        let error = error.to_string();
        assert!(error.contains(status), "{error}");
        assert!(error.contains(mode), "{error}");
    }
}

#[test]
fn audit_rea08_descendant_pipe_does_not_extend_deadline() {
    let host = Path::new(env!("CARGO_BIN_EXE_echode-scan-test-helper"));
    let started = std::time::Instant::now();
    let plugins = echode_plugin_host::scan_with_budget(
        host,
        Path::new("bundle.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect("json");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(plugins[0].id, "dev.audit.scan");

    echode_plugin_host::scan_with_budget(
        host,
        Path::new("noisy.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect("stderr drained");

    let started = std::time::Instant::now();
    let error = echode_plugin_host::scan_with_budget(
        host,
        Path::new("held.clap"),
        std::time::Duration::from_millis(200),
    )
    .expect_err("grandchild holds the pipe");
    let elapsed = started.elapsed();
    assert!(matches!(error, ScanError::TimedOut(_)), "{error:?}");
    let error = error.to_string();
    assert!(error.contains("timed out"), "{error}");
    assert!(
        error.contains(&format!("stdout: {} bytes", PLUGIN_LINE.len())),
        "the descendant fixture must emit its report before the deadline: {error}"
    );
    assert!(
        error.contains("exit status: 0"),
        "the direct child must exit: {error}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "waited {elapsed:?}"
    );

    let started = std::time::Instant::now();
    let error = echode_plugin_host::scan_with_budget(
        host,
        Path::new("hang.clap"),
        std::time::Duration::from_millis(200),
    )
    .expect_err("hang");
    let elapsed = started.elapsed();
    assert!(matches!(error, ScanError::TimedOut(_)), "{error:?}");
    let error = error.to_string();
    assert!(error.contains("timed out"), "{error}");
    assert!(error.contains(": running;"), "{error}");
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");

    let error = echode_plugin_host::scan_with_budget(
        host,
        Path::new("bad.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect_err("utf-8");
    assert!(matches!(error, ScanError::Rejected(_)), "{error:?}");
    assert!(error.to_string().contains("UTF-8"), "{error}");

    let error = echode_plugin_host::scan_with_budget(
        host,
        Path::new("failed.clap"),
        std::time::Duration::from_secs(2),
    )
    .expect_err("exit");
    assert!(matches!(error, ScanError::Rejected(_)), "{error:?}");
    let error = error.to_string();
    assert!(error.contains("failed"), "{error}");
    assert!(error.contains("boom"), "{error}");
}

fn fixture_bundle(path: &str) -> BundleFound {
    BundleFound {
        path: PathBuf::from(path),
        revision: BundleRevision {
            version: "1".to_string(),
            executable_modified: std::time::SystemTime::UNIX_EPOCH,
            executable_size: 1,
        },
    }
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "echode-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// FCA-20: a timeout says nothing about the bundle, so it is not cached as
/// broken and the next refresh scans it again. A rejection is cached.
#[test]
fn contract_a_timed_out_scan_is_not_cached_as_broken() {
    let host = Path::new(env!("CARGO_BIN_EXE_echode-scan-test-helper"));
    let root = scratch("scan-timeout-cache");
    let cache = root.join("scan.jsonl");
    let bundles = ["bundle.clap", "failed.clap", "hang.clap"].map(fixture_bundle);
    // The answering bundles are cached under a generous deadline first, so
    // only the hung one meets the short deadline below, even on a loaded machine.
    echode_plugin_host::refresh_catalog_within(
        host,
        &cache,
        bundles.get(..2).unwrap(),
        std::time::Duration::from_secs(20),
    )
    .expect("refresh");
    let listed = echode_plugin_host::refresh_catalog_within(
        host,
        &cache,
        &bundles,
        std::time::Duration::from_millis(200),
    )
    .expect("refresh");
    assert_eq!(
        listed
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        ["dev.audit.scan"]
    );
    let rows = load_scan_cache(&cache).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.path.as_str(), row.rejected))
            .collect::<Vec<_>>(),
        [("bundle.clap", false), ("failed.clap", true)],
        "only an answer from the helper is cached"
    );
    let [_, failed, hung] = &bundles;
    assert_eq!(
        cached_plugins(&rows, &failed.path, &failed.revision),
        Some(&[][..])
    );
    assert!(
        cached_plugins(&rows, &hung.path, &hung.revision).is_none(),
        "the timed-out bundle must be scanned again"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Rows written before timeouts were told apart may hold a slow bundle.
#[test]
fn an_empty_row_without_the_rejected_mark_is_scanned_again() {
    let root = scratch("scan-old-row");
    let cache = root.join("scan.jsonl");
    std::fs::write(
        &cache,
        concat!(
            r#"{"path":"old.clap","bundle_version":"1","modified_secs":0,"modified_nanos":0,"executable_size":1,"plugins":[]}"#,
            "\n",
            r#"{"path":"new.clap","bundle_version":"1","modified_secs":0,"modified_nanos":0,"executable_size":1,"plugins":[],"rejected":true}"#,
            "\n",
        ),
    )
    .unwrap();
    let rows = load_scan_cache(&cache).unwrap();
    let old = fixture_bundle("old.clap");
    let new = fixture_bundle("new.clap");
    assert!(cached_plugins(&rows, &old.path, &old.revision).is_none());
    assert_eq!(
        cached_plugins(&rows, &new.path, &new.revision),
        Some(&[][..])
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// FCA-20 on the product path: a bundle slower than the first deadline is
/// listed after its slow try, and cached like any other.
#[test]
fn contract_the_live_catalog_lists_a_bundle_slower_than_the_first_deadline() {
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-scan-test-helper"));
    let root = scratch("scan-live-slow");
    let plugins = root.join("plugins");
    for name in ["bundle.clap", "slow.clap"] {
        std::fs::create_dir_all(plugins.join(name).join("Contents/MacOS")).unwrap();
    }
    let cache = root.join("scan.jsonl");
    let live = echode_plugin_host::LiveCatalog::start(
        vec![plugins],
        host,
        cache.clone(),
        CatalogTiming {
            quiet: std::time::Duration::ZERO,
            // The fixture sleeps 500 ms, so its first try always times out.
            first: std::time::Duration::from_millis(200),
            // Far above the fixture's sleep, so a loaded machine still finishes.
            slow: std::time::Duration::from_secs(20),
        },
    )
    .expect("watch");
    let both = wait_for(&live, live.revision(), |rows| {
        rows.iter().any(|row| row.0 == "dev.audit.slow")
            && rows.iter().any(|row| row.0 == "dev.audit.scan")
    });
    assert!(both, "got {:?}", listed(&live.json()));
    drop(live);
    let rows = load_scan_cache(&cache).unwrap();
    assert!(
        rows.iter()
            .any(|row| row.path.ends_with("slow.clap") && row.plugins.len() == 1),
        "{rows:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Newest edit among the test plugin's sources, so a change to any module rebuilds it.
fn newest_source(dir: &Path) -> std::io::Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}

/// A scan helper whose parent is gone must not live on with a hung bundle.
/// The shell starts the helper in the background and exits at once, so the
/// helper is left with launchd, as when the app quits mid-scan.
#[test]
fn contract_an_orphaned_scan_helper_exits_while_the_bundle_hangs() {
    let host = PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"));
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#""$0" --scan "$1" >/dev/null 2>&1 & echo $!"#)
        .arg(&host)
        .arg(plugin_dylib())
        .env("ECHODE_TEST_FAULT", "entry-hang")
        .output()
        .expect("shell");
    assert!(output.status.success(), "{output:?}");
    let pid: i32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("helper pid");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // SAFETY: signal 0 only asks whether the process exists.
    while unsafe { libc::kill(pid, 0) } == 0 {
        if std::time::Instant::now() >= deadline {
            // SAFETY: the pid is the helper this test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("the orphaned scan helper {pid} was still running after 10 s");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
