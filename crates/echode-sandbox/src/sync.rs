//! Wrappers around `os_sync_wait_on_address`.
//!
//! Cross-process waits must use [`SyncFlags::PROCESS`]. The kernel matches
//! those waiters by the shared page, so the two processes may map it at
//! different virtual addresses.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const WORD: usize = 8;
const EINTR: i32 = 4;
const EFAULT: i32 = 14;
const ENOMEM: i32 = 12;
const ETIMEDOUT: i32 = 60;

#[link(name = "System")]
unsafe extern "C" {
    pub(crate) fn os_workgroup_copy_port(workgroup: *mut std::ffi::c_void, port: *mut u32) -> i32;
    pub(crate) fn os_workgroup_create_with_port(
        name: *const std::ffi::c_char,
        port: u32,
    ) -> *mut std::ffi::c_void;
    pub(crate) fn os_workgroup_join(
        workgroup: *mut std::ffi::c_void,
        token: *mut super::workgroup::Token,
    ) -> i32;
    pub(crate) fn os_workgroup_leave(
        workgroup: *mut std::ffi::c_void,
        token: *mut super::workgroup::Token,
    );
    pub(crate) fn os_release(object: *mut std::ffi::c_void);
    pub(crate) fn mach_port_deallocate(task: u32, name: u32) -> i32;
    pub(crate) fn bootstrap_check_in(
        bp: u32,
        service_name: *const std::ffi::c_char,
        sp: *mut u32,
    ) -> i32;
    pub(crate) fn bootstrap_look_up(
        bp: u32,
        service_name: *const std::ffi::c_char,
        sp: *mut u32,
    ) -> i32;
    pub(crate) static bootstrap_port: u32;
    pub(crate) fn mach_msg(
        msg: *mut std::ffi::c_void,
        option: u32,
        send_size: u32,
        recv_size: u32,
        recv_name: u32,
        timeout: u32,
        notify: u32,
    ) -> i32;
    pub(crate) fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;

    fn os_sync_wait_on_address(
        addr: *mut std::ffi::c_void,
        value: u64,
        size: usize,
        flags: u32,
    ) -> i32;
    fn os_sync_wait_on_address_with_timeout(
        addr: *mut std::ffi::c_void,
        value: u64,
        size: usize,
        flags: u32,
        clockid: u32,
        timeout_ns: u64,
    ) -> i32;
    fn os_sync_wake_by_address_any(addr: *mut std::ffi::c_void, size: usize, flags: u32) -> i32;
}

/// `OS_CLOCK_MACH_ABSOLUTE_TIME` from `<os/clock.h>`.
const CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

#[derive(Clone, Copy, Debug)]
pub struct SyncFlags {
    wait: u32,
    wake: u32,
}

impl SyncFlags {
    /// Threads inside one process.
    pub const THREAD: Self = Self { wait: 0, wake: 0 };
    /// Mappings shared with another process (`OS_SYNC_*_SHARED`).
    pub const PROCESS: Self = Self { wait: 1, wake: 1 };
}

#[derive(Debug)]
pub enum WaitError {
    Timeout,
    Os(std::io::Error),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("timed out waiting for the plugin process"),
            Self::Os(error) => write!(f, "os_sync_wait_on_address failed: {error}"),
        }
    }
}

impl std::error::Error for WaitError {}

pub fn wake_one(addr: &AtomicU64, flags: SyncFlags) {
    // SAFETY: `addr` points at an aligned 8-byte atomic that lives for the call.
    unsafe {
        os_sync_wake_by_address_any(addr.as_ptr().cast(), WORD, flags.wake);
    }
}

/// Block until `addr` is no longer `seen`. Returns the new value.
pub fn wait_until_ne(addr: &AtomicU64, seen: u64, flags: SyncFlags) -> Result<u64, WaitError> {
    loop {
        let current = addr.load(Ordering::Acquire);
        if current != seen {
            return Ok(current);
        }
        wait_while_eq(addr, current, flags, None)?;
    }
}

/// Block until `addr` equals `target`, or `timeout` elapses.
pub fn wait_until_eq(
    addr: &AtomicU64,
    target: u64,
    timeout: Duration,
    flags: SyncFlags,
) -> Result<(), WaitError> {
    let deadline = Instant::now() + timeout;
    loop {
        let current = addr.load(Ordering::Acquire);
        if current == target {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(WaitError::Timeout);
        }
        wait_while_eq(addr, current, flags, Some(remaining))?;
    }
}

fn wait_while_eq(
    addr: &AtomicU64,
    expected: u64,
    flags: SyncFlags,
    timeout: Option<Duration>,
) -> Result<(), WaitError> {
    let rc = if let Some(timeout) = timeout {
        let ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX).max(1);
        // SAFETY: `addr` is an aligned 8-byte atomic. The timeout is relative
        // nanoseconds anchored to mach absolute time, per the header.
        unsafe {
            os_sync_wait_on_address_with_timeout(
                addr.as_ptr().cast(),
                expected,
                WORD,
                flags.wait,
                CLOCK_MACH_ABSOLUTE_TIME,
                ns,
            )
        }
    } else {
        // SAFETY: same alignment contract as the timed wait.
        unsafe { os_sync_wait_on_address(addr.as_ptr().cast(), expected, WORD, flags.wait) }
    };

    if rc >= 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(ETIMEDOUT) => Err(WaitError::Timeout),
        Some(EINTR | EFAULT | ENOMEM) => Ok(()),
        _ => Err(WaitError::Os(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn wake_releases_a_waiting_thread() {
        let cell = Arc::new(AtomicU64::new(0));
        let waiting = Arc::clone(&cell);
        let handle = thread::spawn(move || {
            wait_until_ne(&waiting, 0, SyncFlags::THREAD).expect("wait");
        });
        thread::sleep(Duration::from_millis(20));
        cell.store(1, Ordering::Release);
        wake_one(&cell, SyncFlags::THREAD);
        handle.join().expect("joiner");
    }

    #[test]
    fn timeout_tracks_wall_time() {
        let cell = AtomicU64::new(0);
        let started = Instant::now();
        let error = wait_until_eq(&cell, 1, Duration::from_millis(30), SyncFlags::THREAD);
        let elapsed = started.elapsed();
        assert!(matches!(error, Err(WaitError::Timeout)));
        assert!(
            elapsed >= Duration::from_millis(25) && elapsed < Duration::from_millis(200),
            "timeout elapsed {elapsed:?}"
        );
    }
}
