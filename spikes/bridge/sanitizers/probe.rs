//! Deliberately invalid child-process fixtures, only in the sanitizer-control variant.
//! The actual bridge probe is built and preserved without this feature.
//! Each sanitizer must detect both a Rust and a Swift fault before we trust a pass.

use std::ptr;

#[unsafe(no_mangle)]
pub extern "C" fn eb_sanitizer_clean() -> u64 {
    42
}

/// # Safety
/// Intentionally violates memory safety. Execute only as an expected-failing child.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_sanitizer_memory_fault() -> u64 {
    let allocation = Box::into_raw(Box::new(42_u64));
    unsafe {
        drop(Box::from_raw(allocation));
        ptr::read_volatile(allocation)
    }
}

static mut RACE: u64 = 0;

/// # Safety
/// Intentionally races. Execute only as an expected-failing TSan child process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_sanitizer_thread_fault() {
    let thread = std::thread::spawn(|| {
        // Iteration count is this fault fixture's detection budget.
        for value in 0..100_000 {
            unsafe { ptr::write_volatile(&raw mut RACE, value) };
        }
    });
    for value in 0..100_000 {
        unsafe { ptr::write_volatile(&raw mut RACE, value) };
    }
    thread.join().unwrap();
}
