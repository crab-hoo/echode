use super::*;
use std::sync::atomic::AtomicU32;

/// Installs the hook for this test binary at a log under the temp directory,
/// never the user's `~/Library/Logs`. Every test that needs the hook calls it.
pub(crate) fn test_log() -> &'static Path {
    static LOG: OnceLock<PathBuf> = OnceLock::new();
    LOG.get_or_init(|| {
        let path = std::env::temp_dir()
            .join(format!("echode-panic-log-{}", std::process::id()))
            .join("panic.ndjson");
        keep_panic_log(Some(path.clone()));
        path
    })
}

fn logged(marker: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(test_log())
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(marker))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_caught_callback_panic_keeps_its_location_and_stops_later_blocks() {
    test_log();
    let panic = CallbackPanic::new("test callback");
    let ran = AtomicU32::new(0);
    let block = 7;
    assert!(panic.run(|| {
        ran.fetch_add(1, Ordering::Relaxed);
    }));
    assert!(!panic.run(|| panic!("caught marker {block}")));
    assert!(!panic.run(|| {
        ran.fetch_add(1, Ordering::Relaxed);
    }));
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    let reason = panic.reason().unwrap();
    assert!(reason.contains("panic_log_tests.rs:"), "{reason}");
    assert!(reason.ends_with("caught marker 7"), "{reason}");
    assert_eq!(panic.reason(), Some(reason));
    let lines = logged("caught marker 7");
    assert_eq!(lines.len(), 1, "reported once: {lines:?}");
    assert_eq!(lines[0]["source"], "test callback");
    assert!(lines[0].get("backtrace").is_none());
}

#[test]
fn a_callback_that_never_panicked_has_no_reason_and_logs_nothing() {
    test_log();
    let panic = CallbackPanic::new("quiet callback");
    assert!(panic.run(|| {}));
    assert!(!panic.panicked());
    assert_eq!(panic.reason(), None);
}

#[test]
fn a_long_callback_message_is_cut_at_a_character_boundary() {
    test_log();
    let panic = CallbackPanic::new("test callback");
    let message = format!("long marker {}", "é".repeat(TEXT));
    assert!(!panic.run(|| std::panic::panic_any(message.clone())));
    let reason = panic.reason().unwrap();
    assert!(
        reason.len() <= TEXT && reason.len() > TEXT - 2,
        "{}",
        reason.len()
    );
    assert!(reason.contains("long marker é"));
}

#[test]
fn another_threads_panic_is_appended_with_its_thread_and_backtrace() {
    test_log();
    let joined = std::thread::Builder::new()
        .name("panicking-worker".into())
        .spawn(|| panic!("thread marker {}", 11))
        .unwrap()
        .join();
    assert!(joined.is_err());
    let lines = logged("thread marker 11");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["source"], "thread");
    assert_eq!(lines[0]["thread"], "panicking-worker");
    assert!(
        lines[0]["message"]
            .as_str()
            .unwrap()
            .contains("panic_log_tests.rs:")
    );
    assert!(lines[0]["backtrace"].is_string());
}

#[test]
fn a_full_log_moves_aside_before_the_next_line() {
    let directory =
        std::env::temp_dir().join(format!("echode-panic-rotate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("panic.ndjson");
    std::fs::write(&path, vec![b'x'; LOG_LIMIT as usize]).unwrap();
    let entry = Entry {
        time_ms: 1,
        source: "test",
        thread: None,
        message: "after rotation",
        backtrace: None,
    };
    append(&path, &entry);
    assert_eq!(
        std::fs::metadata(directory.join("panic.1.ndjson"))
            .unwrap()
            .len(),
        LOG_LIMIT
    );
    let line = std::fs::read_to_string(&path).unwrap();
    assert!(
        line.ends_with("\"message\":\"after rotation\"}\n"),
        "{line}"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
