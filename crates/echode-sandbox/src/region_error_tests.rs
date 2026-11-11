use super::*;
use crate::SessionError;
use std::error::Error;

#[test]
fn shared_creation_keeps_the_native_truncate_cause() {
    // This layout only exercises the off_t boundary: its mapping length fits
    // usize but becomes negative for ftruncate, before any pages are mapped.
    let layout = SessionLayout {
        max_frames: u32::MAX,
        input_channels: (1 << 28) + 1,
        output_channels: 0,
        input_events: 0,
        output_events: 0,
    };
    assert!(layout.mapping_len().unwrap() > i64::MAX as usize);
    let error = MappedRegion::create_shared(layout)
        .err()
        .expect("ftruncate must reject the size");
    assert!(error.to_string().starts_with("ftruncate: "), "{error}");
    let source = error
        .source()
        .and_then(|cause| cause.downcast_ref::<io::Error>());
    assert_eq!(
        source.and_then(io::Error::raw_os_error),
        Some(libc::EINVAL),
        "shared creation must retain the native errno: {error:?}"
    );
    let session = SessionError::from(error);
    assert!(matches!(
        session,
        SessionError::Region(RegionError::SharedMemory {
            operation: SharedMemoryOperation::Truncate,
            error,
        }) if error.raw_os_error() == Some(libc::EINVAL)
    ));
}

#[test]
fn shared_memory_errors_keep_operation_errno_and_display() {
    for (operation, prefix, code) in [
        (SharedMemoryOperation::Open, "shm_open", libc::EACCES),
        (SharedMemoryOperation::Unlink, "shm_unlink", libc::ENOENT),
        (SharedMemoryOperation::Truncate, "ftruncate", libc::EINVAL),
    ] {
        let original = io::Error::from_raw_os_error(code);
        let expected = format!("{prefix}: {original}");
        let error = RegionError::SharedMemory {
            operation,
            error: original,
        };
        assert_eq!(error.to_string(), expected);
        let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.raw_os_error(), Some(code));
        assert_eq!(source.kind(), io::Error::from_raw_os_error(code).kind());
    }
}

fn name() -> CString {
    CString::new(format!(
        "/echode-test-{}-{}",
        std::process::id(),
        SHM_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
    .unwrap()
}

#[test]
fn native_open_collision_keeps_errno_and_existing_object() {
    let name = name();
    // SAFETY: the name is NUL-terminated. A successful call owns a new fd.
    let fd = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600 as libc::c_int,
        )
    };
    assert!(fd >= 0, "{}", io::Error::last_os_error());
    // SAFETY: this test exclusively owns the new descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let result = shm_file_named(&name, 4096);
    // Unlink before assertions so the test leaves no named object on failure.
    // Its success also proves the collision did not unlink the existing object.
    // SAFETY: `name` is a NUL-terminated C string.
    let unlinked = unsafe { libc::shm_unlink(name.as_ptr()) };
    assert_eq!(unlinked, 0);
    let error = result.unwrap_err();
    assert!(matches!(
        error,
        RegionError::SharedMemory {
            operation: SharedMemoryOperation::Open,
            error,
        } if error.raw_os_error() == Some(libc::EEXIST)
    ));
    assert!(file.metadata().is_ok());
}

#[test]
fn shared_file_unlinks_name_sizes_file_and_sets_close_on_exec() {
    // This size is a test fixture, not a mapping or session limit.
    let name = name();
    let file = shm_file_named(&name, 4096).unwrap();
    // macOS may round the shared object length up to a page boundary.
    assert!(file.metadata().unwrap().len() >= 4096);
    // SAFETY: file owns the descriptor for this query.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::FD_CLOEXEC, 0);
    // SAFETY: the valid name is queried without creating another object.
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDWR, 0 as libc::c_int) };
    let error = io::Error::last_os_error();
    if fd >= 0 {
        // SAFETY: clean up an unexpected successful open before failing.
        drop(unsafe { File::from_raw_fd(fd) });
        // SAFETY: `name` is a NUL-terminated C string.
        unsafe { libc::shm_unlink(name.as_ptr()) };
    }
    assert_eq!(fd, -1);
    assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
}
