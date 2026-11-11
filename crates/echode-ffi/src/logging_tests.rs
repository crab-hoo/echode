use std::ffi::CString;

use super::*;

#[test]
fn the_app_writes_records_at_its_levels_and_bad_arguments_are_refused() {
    let capture = echode_log::testing::capture();
    let category = CString::new("arrange").unwrap();
    let message = CString::new("renderer failed: device lost").unwrap();
    // SAFETY: both strings are NUL-terminated and live for the calls.
    unsafe {
        assert!(echode_log_write(
            ECHODE_LOG_ERROR,
            category.as_ptr(),
            message.as_ptr()
        ));
        assert!(!echode_log_write(99, category.as_ptr(), message.as_ptr()));
        assert!(!echode_log_write(
            ECHODE_LOG_INFO,
            std::ptr::null(),
            message.as_ptr()
        ));
        assert!(!echode_log_write(
            ECHODE_LOG_INFO,
            category.as_ptr(),
            std::ptr::null()
        ));
    }
    let records = capture.on_this_thread();
    let [record] = records.as_slice() else {
        panic!("only the valid call logs: {records:?}");
    };
    assert_eq!(record["level"], "error");
    assert_eq!(record["target"], "app::arrange");
    assert_eq!(record["message"], "renderer failed: device lost");
}

#[test]
fn appended_lines_share_the_rotation_rule_and_bad_arguments_write_nothing() {
    let directory = std::env::temp_dir().join(format!(
        "echode-ffi-append-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    let file = directory.join("audition.ndjson");
    let path = CString::new(file.to_str().unwrap()).unwrap();
    let line = b"{\"request\":1}\n";
    // SAFETY: the path is NUL-terminated and `line` is readable for its length.
    unsafe {
        assert!(echode_log_append_lines(
            path.as_ptr(),
            line.as_ptr(),
            line.len(),
            ECHODE_LOG_FILE_LIMIT
        ));
        assert!(echode_log_append_lines(
            path.as_ptr(),
            std::ptr::null(),
            0,
            ECHODE_LOG_FILE_LIMIT
        ));
        assert!(!echode_log_append_lines(
            path.as_ptr(),
            std::ptr::null(),
            1,
            ECHODE_LOG_FILE_LIMIT
        ));
        assert!(!echode_log_append_lines(
            std::ptr::null(),
            line.as_ptr(),
            line.len(),
            ECHODE_LOG_FILE_LIMIT
        ));
    }
    assert_eq!(std::fs::read(&file).unwrap(), line);
    std::fs::write(&file, vec![b'x'; echode_log::FILE_LIMIT as usize]).unwrap();
    // SAFETY: as above.
    unsafe {
        assert!(echode_log_append_lines(
            path.as_ptr(),
            line.as_ptr(),
            line.len(),
            ECHODE_LOG_FILE_LIMIT
        ));
    }
    assert_eq!(std::fs::read(&file).unwrap(), line);
    assert_eq!(
        std::fs::metadata(echode_log::previous(&file))
            .unwrap()
            .len(),
        echode_log::FILE_LIMIT
    );
    std::fs::remove_dir_all(directory).unwrap();
}
