//! Detects heap allocation while [`ForbidAlloc`] is alive.
//!
//! The helper is a separate process, so the DAW's allocator cannot see it.
//! Debug builds of this binary install [`GuardAlloc`]. Release builds skip the
//! check so measurements stay unchanged. The guard aborts instead of panicking,
//! because formatting a panic would allocate again.
//! A thread that is already panicking may allocate: the panic formats its
//! message and boxes its payload, and refusing that would turn a callback's
//! caught panic into an abort with no reason (ADR 0238).

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

/// Aborting allocator used by debug tests and by the debug helper.
#[cfg(debug_assertions)]
pub struct GuardAlloc;

#[cfg(debug_assertions)]
// SAFETY: every method forwards the same arguments to `System`, which upholds the `GlobalAlloc`
// contract; `refuse` only aborts and never allocates.
unsafe impl std::alloc::GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        refuse();
        // SAFETY: the caller's contract for `alloc` is `System.alloc`'s.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        refuse();
        // SAFETY: the caller's contract for `dealloc` is `System.dealloc`'s, and every block came
        // from `System` through this allocator.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        refuse();
        // SAFETY: the caller's contract for `realloc` is `System.realloc`'s, and every block came
        // from `System` through this allocator.
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
