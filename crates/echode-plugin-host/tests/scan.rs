//! The helper reads CLAP descriptors. The test plugin is not instantiated.

use echode_plugin_host::{scan_descriptors, scan_report};
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
    assert_eq!(
        scan_report(&plugins),
        "{\"id\":\"dev.echode.test-gain\",\"name\":\"Echode Test Gain\"}\n"
    );
}
