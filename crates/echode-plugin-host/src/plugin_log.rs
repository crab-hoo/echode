//! Bounded, non-waiting CLAP diagnostics from arbitrary plugin threads.
//!
//! Each producer claims one slot once. Contention or a full slot drops a record
//! rather than retrying. Only a non-audio consumer formats or writes messages.
use clap_sys::ext::log::*;
use std::cell::UnsafeCell;
use std::ffi::c_char;
use std::io::Write as _;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

// These bounds limit diagnostics only, never plugin instances or audio ports.
const SLOTS: usize = 64;
const TEXT_BYTES: usize = 1024;
const EMPTY: u8 = 0;
const BUSY: u8 = 1;
const READY: u8 = 2;

#[derive(Clone, Copy)]
struct Record {
    severity: clap_log_severity,
    text: [u8; TEXT_BYTES],
    len: usize,
    truncated: bool,
}

struct Slot {
    state: AtomicU8,
    record: UnsafeCell<MaybeUninit<Record>>,
}

// SAFETY: only the thread that changed EMPTY or READY to BUSY accesses the
// payload. Publishing READY and EMPTY releases it to the next acquiring owner.
unsafe impl Sync for Slot {}

pub(crate) struct PluginLog {
    plugin: OnceLock<Box<str>>,
    slots: Box<[Slot]>,
    next: AtomicUsize,
    dropped: AtomicU64,
}

impl PluginLog {
    pub(crate) fn new() -> Self {
        Self {
            plugin: OnceLock::new(),
            slots: (0..SLOTS)
                .map(|_| Slot {
                    state: AtomicU8::new(EMPTY),
                    record: UnsafeCell::new(MaybeUninit::uninit()),
                })
                .collect(),
            next: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Set before creating the instance, including its initialization callbacks.
    pub(crate) fn identify(&self, plugin: &std::ffi::CStr) {
        let _ = self
            .plugin
            .set(plugin.to_string_lossy().into_owned().into());
    }

    /// Any plugin thread. No allocation, lock, formatting, I/O, or retry.
    ///
    /// # Safety
    /// A non-null `message` must point to a NUL-terminated string valid for this
    /// call, as required by CLAP. The prefix copy examines at most TEXT_BYTES.
    pub(crate) unsafe fn push(&self, severity: clap_log_severity, message: *const c_char) {
        if message.is_null() {
            return;
        }
        let position = self.next.fetch_add(1, Ordering::Relaxed) % SLOTS;
        let Some(slot) = self.slots.get(position) else {
            return;
        };
        if slot
            .state
            .compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut record = Record {
            severity,
            text: [0; TEXT_BYTES],
            len: 0,
            truncated: true,
        };
        for (offset, byte) in record.text.iter_mut().enumerate() {
            // SAFETY: CLAP keeps the string valid for this call. We stop at
            // its first NUL and never inspect beyond the diagnostic prefix.
            let value = unsafe { *message.add(offset) } as u8;
            if value == 0 {
                record.truncated = false;
                break;
            }
            *byte = value;
            record.len += 1;
        }
        // SAFETY: the EMPTY -> BUSY transition gave this producer sole access.
        unsafe { (*slot.record.get()).write(record) };
        slot.state.store(READY, Ordering::Release);
    }

    /// A bounded scan, not an unbounded drain under a continuously logging plugin.
    /// Delivery is best effort, with no ordering guarantee across plugin threads.
    fn drain(&self, mut consume: impl FnMut(Record)) -> u64 {
        for slot in &self.slots {
            if slot
                .state
                .compare_exchange(READY, BUSY, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                continue;
            }
            // SAFETY: READY -> BUSY gives this consumer the initialized record.
            // Copy it before publishing EMPTY so formatting never owns a slot.
            let record = unsafe { (*slot.record.get()).assume_init_read() };
            slot.state.store(EMPTY, Ordering::Release);
            consume(record);
        }
        self.dropped.swap(0, Ordering::Relaxed)
    }

    /// Control thread only (or the creating thread after all workers join).
    pub(crate) fn emit(&self) {
        let plugin = self.plugin.get().map_or("<unknown>", |id| id.as_ref());
        let dropped = self.drain(|record| {
            let text = String::from_utf8_lossy(record.text.get(..record.len).unwrap_or_default());
            // Debug string formatting escapes newlines and terminal controls.
            let _ = writeln!(
                std::io::stderr().lock(),
                "clap.log plugin={plugin:?} severity={} code={} truncated={} message={text:?}",
                severity_name(record.severity),
                record.severity,
                record.truncated
            );
        });
        if dropped != 0 {
            let _ = writeln!(
                std::io::stderr().lock(),
                "clap.log plugin={plugin:?} dropped={dropped}"
            );
        }
    }
}

impl Drop for PluginLog {
    fn drop(&mut self) {
        // The bridge's creating thread retains it until the scoped workers
        // join and the plugin is destroyed. This also flushes launch failures.
        self.emit();
    }
}

fn severity_name(severity: clap_log_severity) -> &'static str {
    match severity {
        CLAP_LOG_DEBUG => "debug",
        CLAP_LOG_INFO => "info",
        CLAP_LOG_WARNING => "warning",
        CLAP_LOG_ERROR => "error",
        CLAP_LOG_FATAL => "fatal",
        CLAP_LOG_HOST_MISBEHAVING => "host-misbehaving",
        CLAP_LOG_PLUGIN_MISBEHAVING => "plugin-misbehaving",
        _ => "unknown",
    }
}

#[cfg(test)]
#[path = "plugin_log_tests.rs"]
mod tests;
