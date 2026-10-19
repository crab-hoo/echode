use super::*;

fn bundle(name: &str, secs: u64) -> BundleFound {
    BundleFound {
        path: PathBuf::from(name),
        revision: BundleRevision {
            version: String::new(),
            executable_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            executable_size: secs,
        },
    }
}

#[test]
fn a_bundle_is_not_due_until_the_quiet_period_ends() {
    let mut catalog = PluginCatalog::new(Duration::from_secs(2));
    let start = Instant::now();
    catalog.observe(start, &[bundle("A.clap", 1)]);
    assert!(catalog.due(start).is_empty());
    assert!(catalog.due(start + Duration::from_secs(1)).is_empty());
    assert_eq!(
        catalog.due(start + Duration::from_secs(2)),
        vec![PathBuf::from("A.clap")]
    );
}

#[test]
fn removing_a_bundle_drops_it_and_a_failed_scan_waits_for_a_new_write() {
    let mut catalog = PluginCatalog::new(Duration::ZERO);
    let start = Instant::now();
    catalog.observe(start, &[bundle("A.clap", 1), bundle("B.vst3", 1)]);
    catalog.note_failed(Path::new("A.clap"));
    assert_eq!(catalog.due(start), vec![PathBuf::from("B.vst3")]);

    catalog.observe(start, &[bundle("A.clap", 1)]);
    assert!(catalog.due(start).is_empty());
    assert!(catalog.contains(Path::new("A.clap")));
    assert!(!catalog.contains(Path::new("B.vst3")));

    catalog.observe(start + Duration::from_secs(3), &[bundle("A.clap", 2)]);
    assert_eq!(
        catalog.due(start + Duration::from_secs(3)),
        vec![PathBuf::from("A.clap")]
    );
}

#[test]
fn a_finished_scan_is_not_due_again() {
    let mut catalog = PluginCatalog::new(Duration::ZERO);
    let start = Instant::now();
    catalog.observe(start, &[bundle("A.clap", 1)]);
    catalog.note_scanned(
        Path::new("A.clap"),
        vec![crate::ScannedPlugin {
            id: "dev.echode.test-gain".into(),
            name: "Echode Test Gain".into(),
            vendor: "Echode".into(),
            version: "0.0.0".into(),
            features: vec!["audio-effect".into()],
            url: String::new(),
        }],
    );
    assert!(catalog.due(start).is_empty());
    assert_eq!(
        catalog.plugins(Path::new("A.clap")).unwrap()[0].id,
        "dev.echode.test-gain"
    );
}

#[test]
fn a_temp_folder_lists_only_plugin_bundles() {
    let root = std::env::temp_dir().join(format!(
        "echode-scan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("Noise.clap")).unwrap();
    std::fs::create_dir_all(root.join("Drum.vst3")).unwrap();
    std::fs::write(root.join("notes.txt"), b"no").unwrap();
    let listed = list_bundles(&root);
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|bundle| bundle.path.ends_with("Noise.clap"))
    );
    assert!(
        listed
            .iter()
            .any(|bundle| bundle.path.ends_with("Drum.vst3"))
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_new_bundle_version_changes_the_revision_without_touching_the_executable() {
    let root = std::env::temp_dir().join(format!(
        "echode-rev-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let bundle = root.join("Synth.clap");
    std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
    std::fs::write(bundle.join("Contents/MacOS/Plugin"), b"same").unwrap();
    std::fs::write(
            bundle.join("Contents/Info.plist"),
            br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>"#,
        )
        .unwrap();
    let first = bundle_revision(&bundle);
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>2</string>
</dict></plist>"#,
    )
    .unwrap();
    let second = bundle_revision(&bundle);
    let _ = std::fs::remove_dir_all(root);
    assert_eq!(first.version, "1");
    assert_eq!(second.version, "2");
    assert_ne!(first, second);
    assert_eq!(first.executable_size, second.executable_size);
}

#[test]
fn a_binary_plist_version_is_part_of_the_revision() {
    let root = std::env::temp_dir().join(format!(
        "echode-bplist-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let bundle = root.join("Synth.clap");
    let plist = bundle.join("Contents/Info.plist");
    std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
    std::fs::write(bundle.join("Contents/MacOS/Plugin"), b"same").unwrap();
    std::fs::write(
        &plist,
        br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Plugin</string>
<key>CFBundleVersion</key><string>9</string>
</dict></plist>"#,
    )
    .unwrap();
    let converted = std::process::Command::new("plutil")
        .args(["-convert", "binary1", plist.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(converted.success());
    let revision = bundle_revision(&bundle);
    let _ = std::fs::remove_dir_all(root);
    assert_eq!(revision.version, "9");
    assert_eq!(revision.executable_size, 4);
}
