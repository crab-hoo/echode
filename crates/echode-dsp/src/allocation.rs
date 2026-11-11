//! Detects heap allocation while [`ForbidAlloc`] is alive.
//!
//! The caller installs [`GuardAlloc`] in debug binaries. Release builds skip
//! the check. An already panicking thread may allocate, so a caught callback
//! panic can retain its reason instead of aborting in the allocator (ADR 0238).

#[cfg(debug_assertions)]
use std::cell::Cell;

#[cfg(debug_assertions)]
std::thread_local! {
    static FORBID: Cell<bool> = const { Cell::new(false) };
}

/// While this value is alive on a thread, allocating or freeing aborts the process.
pub struct ForbidAlloc {
    previous: bool,
}

impl ForbidAlloc {
    pub fn enter() -> Self {
        #[cfg(debug_assertions)]
        let previous = FORBID.with(|flag| flag.replace(true));
        #[cfg(not(debug_assertions))]
        let previous = false;
        Self { previous }
    }
}

impl Drop for ForbidAlloc {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        FORBID.with(|flag| flag.set(self.previous));
        #[cfg(not(debug_assertions))]
        let _ = self.previous;
    }
}

/// Aborting allocator used by debug tests and by debug binaries that opt in.
#[cfg(debug_assertions)]
pub struct GuardAlloc;

#[cfg(debug_assertions)]
// SAFETY: every method forwards the same arguments to `System`, which upholds the `GlobalAlloc`
// contract; `refuse` only aborts and never allocates.
unsafe impl std::alloc::GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        refuse();
        // SAFETY: layout is supplied by the GlobalAlloc caller.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        refuse();
        // SAFETY: the caller returns an allocation with its original layout.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        refuse();
        // SAFETY: the caller supplies a live allocation and a valid new size.
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(debug_assertions)]
fn refuse() {
    if FORBID.with(|flag| flag.get()) && !std::thread::panicking() {
        std::process::abort();
    }
}

#[cfg(all(test, debug_assertions))]
#[global_allocator]
static ALLOCATOR: GuardAlloc = GuardAlloc;
