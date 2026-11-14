//! Main run-loop notifications and their realtime-to-control handoff.

use objc2_core_foundation::{CFRunLoop, CFRunLoopSource};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

#[derive(Default)]
pub(crate) struct MainSignal {
    source: AtomicPtr<CFRunLoopSource>,
    run_loop: AtomicPtr<CFRunLoop>,
    pending: AtomicBool,
}

impl MainSignal {
    /// # Safety
    /// Both objects must remain retained until all callers have stopped.
    pub(crate) unsafe fn install(&self, source: &CFRunLoopSource, run_loop: &CFRunLoop) {
        self.source
            .store(ptr::from_ref(source).cast_mut(), Ordering::Release);
        self.run_loop
            .store(ptr::from_ref(run_loop).cast_mut(), Ordering::Release);
    }

    /// Returns whether the control thread must relay the notification.
    pub(crate) fn signal(&self, realtime: bool) -> bool {
        if realtime {
            // CoreFoundation takes locks, even when only signalling a source.
            self.pending.store(true, Ordering::Release);
            return true;
        }
        let source = self.source.load(Ordering::Acquire);
        let run_loop = self.run_loop.load(Ordering::Acquire);
        if !source.is_null() && !run_loop.is_null() {
            // SAFETY: `install` requires both objects to outlive all callers.
            // CoreFoundation allows signalling and waking from other threads.
            unsafe {
                (*source).signal();
                (*run_loop).wake_up();
            }
        }
        false
    }

    /// Control-thread only. Clear before signalling so a racing request remains pending.
    pub(crate) fn relay(&self) {
        if self.pending.swap(false, Ordering::AcqRel) {
            self.signal(false);
        }
    }

    pub(crate) fn stop(&self) {
        let run_loop = self.run_loop.load(Ordering::Acquire);
        if !run_loop.is_null() {
            // SAFETY: `install` requires the run loop to outlive all callers.
            // Stopping the run loop is allowed from other threads, outside realtime work.
            unsafe { (*run_loop).stop() };
        }
    }
}

#[cfg(test)]
#[path = "main_signal_tests.rs"]
mod tests;
