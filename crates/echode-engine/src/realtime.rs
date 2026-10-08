//! Detects heap allocation on the audio thread in debug builds.
//!
//! Release builds do not install this allocator, so measurements are unchanged.
//! The guard aborts instead of panicking, because formatting a panic would
//! allocate again.

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

#[cfg(debug_assertions)]
struct GuardAlloc;

#[cfg(debug_assertions)]
unsafe impl std::alloc::GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        refuse();
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        refuse();
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        refuse();
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[cfg(debug_assertions)]
fn refuse() {
    if FORBID.with(|flag| flag.get()) {
        std::process::abort();
    }
}

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: GuardAlloc = GuardAlloc;
