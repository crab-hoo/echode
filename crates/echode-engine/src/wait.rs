//! Realtime wait/wake for threads in one process.
//!
//! Uses `os_sync_wait_on_address` without the shared-memory flag. The audio
//! thread never takes a lock.

use std::sync::atomic::{AtomicU64, Ordering};

const WORD: usize = 8;
const EINTR: i32 = 4;
const EFAULT: i32 = 14;
const ENOMEM: i32 = 12;

#[link(name = "System")]
unsafe extern "C" {
    fn os_sync_wait_on_address(
        addr: *mut std::ffi::c_void,
        value: u64,
        size: usize,
        flags: u32,
    ) -> i32;
    fn os_sync_wake_by_address_all(addr: *mut std::ffi::c_void, size: usize, flags: u32) -> i32;
}

pub fn wake_all(addr: &AtomicU64) {
    // SAFETY: `addr` is an aligned 8-byte atomic that stays mapped for the call.
    unsafe {
        os_sync_wake_by_address_all(addr.as_ptr().cast(), WORD, 0);
    }
}

pub fn wait_until_ne(addr: &AtomicU64, seen: u64) -> u64 {
    loop {
        let current = addr.load(Ordering::Acquire);
        if current != seen {
            return current;
        }
        wait_while_eq(addr, current);
    }
}

pub fn wait_until_ge(addr: &AtomicU64, target: u64) {
    loop {
        let current = addr.load(Ordering::Acquire);
        if current >= target {
            return;
        }
        wait_while_eq(addr, current);
    }
}

fn wait_while_eq(addr: &AtomicU64, expected: u64) {
    // SAFETY: same alignment contract as `wake_all`. A spurious wake retries.
    let rc = unsafe { os_sync_wait_on_address(addr.as_ptr().cast(), expected, WORD, 0) };
    if rc < 0 {
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(EINTR | EFAULT | ENOMEM) => {}
            _ => {
                // A permanent error on the audio thread cannot be logged with a lock.
                // Spin so the block still finishes instead of sleeping forever.
            }
        }
    }
}
