use super::*;
use std::any::Any;
use std::error::Error;
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("echode-ffi-catalog-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

// Accept String too, so the old boundary fails an assertion, not compilation.
fn typed(error: &dyn Any) -> &CatalogError {
    error
        .downcast_ref::<CatalogError>()
        .expect("plugin catalog lost its typed cause")
}

#[test]
fn catalog_failures_keep_their_typed_causes() {
    const CHILD: &str = "ECHODE_FFI_CATALOG_CASE";
    if let Ok(case) = std::env::var(CHILD) {
        match case.as_str() {
            "missing-host" => {
                let path = PathBuf::from(std::env::var_os("ECHODE_PLUGIN_HOST").unwrap());
                let error = plugins_json().unwrap_err();
                assert!(
                    matches!(typed(&error), CatalogError::MissingHost(Some(found)) if found == &path)
                );
                assert_eq!(
                    error.to_string(),
                    format!("plugin host is missing ({})", path.display())
                );
                let error = start_watch().unwrap_err();
                assert!(
                    matches!(typed(&error), CatalogError::MissingHost(Some(found)) if found == &path)
                );
                assert_eq!(
                    error.to_string(),
                    format!("plugin host is missing ({})", path.display())
                );
                assert!(watch_waiter().is_none());
                assert_eq!(wait_watch(17, 0), 17);
                let mut written = usize::MAX;
                assert_eq!(
                    unsafe { crate::echode_plugin_catalog(std::ptr::null_mut(), 0, &mut written) },
                    -1
                );
                assert_eq!(written, 0);
                assert_eq!(unsafe { crate::echode_plugin_watch_start() }, -1);
            }
            "cache-parent" | "cache-file" => {
                let cache = cache_path();
                let expected = if case == "cache-parent" {
                    std::fs::create_dir_all(cache.parent().unwrap()).unwrap_err()
                } else {
                    std::fs::write(&cache, b"").unwrap_err()
                };
                let error = plugins_json().unwrap_err();
                let CatalogError::Catalog(echode_plugin_host::CatalogError::Cache(cause)) =
                    typed(&error)
                else {
                    panic!("cache failure lost its I/O cause: {error:?}");
                };
                assert_eq!(cause.raw_os_error(), expected.raw_os_error());
                assert_eq!(cause.kind(), expected.kind());
                assert_eq!(error.to_string(), expected.to_string());
            }
            "empty" => {
                assert_eq!(plugins_json().unwrap(), b"[]");
                let mut written = 0;
                assert_eq!(
                    unsafe { crate::echode_plugin_catalog(std::ptr::null_mut(), 0, &mut written) },
                    1
                );
                assert_eq!(written, 2);
                // This buffer covers the empty-list fixture, not a product limit.
                let mut bytes = [0_u8; 2];
                assert_eq!(
                    unsafe {
                        crate::echode_plugin_catalog(bytes.as_mut_ptr(), bytes.len(), &mut written)
                    },
                    0
                );
                assert_eq!(&bytes, b"[]");
                start_watch().unwrap();
                let revision = wait_watch(0, 0);
                assert!(revision > 0);
                start_watch().unwrap();
                assert_eq!(wait_watch(revision, 0), revision);
                assert_eq!(plugins_json().unwrap(), b"[]");
                assert_eq!(unsafe { crate::echode_plugin_watch_start() }, 0);
                let watch = WATCH.lock().unwrap().take().unwrap();
                drop(watch);
            }
            "cached" => {
                let bundle = PathBuf::from(std::env::var_os("ECHODE_CLAP_ROOTS").unwrap())
                    .join("Fixture.clap");
                std::fs::create_dir_all(&bundle).unwrap();
                let plugin = echode_plugin_host::ScannedPlugin {
                    id: "dev.echode.fixture".into(),
                    name: "Quoted \"name\" 日本語".into(),
                    vendor: "Fixture".into(),
                    version: "1.0".into(),
                    features: vec!["audio-effect".into()],
                    url: String::new(),
                };
                let entry = echode_plugin_host::cache_entry(&bundle, vec![plugin.clone()]);
                echode_plugin_host::store_scan_cache(&cache_path(), &[entry]).unwrap();
                let actual: serde_json::Value =
                    serde_json::from_slice(&plugins_json().unwrap()).unwrap();
                let mut expected = serde_json::to_value(plugin).unwrap();
                expected["fresh"] = false.into();
                assert_eq!(actual, serde_json::json!([expected]));
            }
            _ => panic!("unknown catalog fixture: {case}"),
        }
        return;
    }

    // Each child owns its environment, scan roots and watch state.
    // The fake host is not executable; no installed bundles are visited.
    for case in [
        "missing-host",
        "cache-parent",
        "cache-file",
        "empty",
        "cached",
    ] {
        let fixture = Fixture::new();
        let host = fixture.0.join("host");
        if case != "missing-host" {
            std::fs::write(&host, b"not an executable").unwrap();
        }
        let cache = match case {
            "cache-parent" => {
                let blocked = fixture.0.join("blocked");
                std::fs::write(&blocked, b"preserve this file").unwrap();
                blocked.join("scan.jsonl")
            }
            "cache-file" => {
                let cache = fixture.0.join("scan.jsonl");
                std::fs::create_dir(&cache).unwrap();
                cache
            }
            _ => fixture.0.join("scan.jsonl"),
        };
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "catalog::tests::catalog_failures_keep_their_typed_causes",
                "--exact",
                "--nocapture",
            ])
            .env(CHILD, case)
            .env("ECHODE_PLUGIN_HOST", &host)
            .env("ECHODE_PLUGIN_CACHE", &cache)
            .env("HOME", &fixture.0)
            .env("CLAP_PATH", fixture.0.join("plugins"))
            .env("ECHODE_CLAP_ROOTS", fixture.0.join("plugins"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "catalog fixture {case}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if case == "cache-parent" {
            assert_eq!(
                std::fs::read(fixture.0.join("blocked")).unwrap(),
                b"preserve this file"
            );
        }
    }
}

#[test]
fn catalog_error_sources_and_messages_remain_available() {
    let error = CatalogError::MissingHost(None);
    assert_eq!(error.to_string(), "plugin host is missing");
    assert!(error.source().is_none());

    for wrap in [
        echode_plugin_host::CatalogError::Cache,
        echode_plugin_host::CatalogError::Thread,
    ] {
        let cause = std::io::Error::from_raw_os_error(libc::EACCES);
        let expected = cause.to_string();
        let error = CatalogError::Catalog(wrap(cause));
        assert_eq!(error.to_string(), expected);
        assert!(
            error
                .source()
                .unwrap()
                .is::<echode_plugin_host::CatalogError>()
        );
    }

    let cause = serde_json::from_slice::<serde_json::Value>(b"{\n invalid}").unwrap_err();
    let expected = cause.to_string();
    let error = CatalogError::Json(cause);
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<serde_json::Error>()
        .unwrap();
    assert_eq!(source.line(), 2);
    assert!(source.is_syntax());
    assert_eq!(error.to_string(), expected);
}
