//! CLAP main-thread timers. The helper's run loop waits for the next deadline;
//! neither registration nor dispatch touches the audio thread or a worker lock.
use crate::threads::{EchodeHost, host_is_main_thread};
use clack_host::extensions::prelude::*;
use clack_host::prelude::PluginInstance;
use clap_sys::ext::timer_support::{
    CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support,
};
use clap_sys::host::clap_host;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::time::{Duration, Instant};

struct Timer {
    period: Duration,
    next: Instant,
}

/// Owned by one instance's main-thread handler and dropped with that instance.
#[derive(Default)]
pub(crate) struct Timers {
    entries: BTreeMap<u32, Timer>,
    // Never recycle an ID: a callback may cancel a later ID in the due snapshot
    // and register a replacement. CLAP reserves u32::MAX as the invalid ID.
    next_id: u32,
}

impl Timers {
    fn register(&mut self, period_ms: u32, now: Instant) -> Option<u32> {
        if period_ms == 0 || self.next_id == u32::MAX {
            return None;
        }
        let period = Duration::from_millis(u64::from(period_ms));
        let next = now.checked_add(period)?;
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(id, Timer { period, next });
        Some(id)
    }

    fn unregister(&mut self, id: u32) -> bool {
        self.entries.remove(&id).is_some()
    }

    fn wait(&self, now: Instant, maximum: Duration) -> Duration {
        self.entries
            .values()
            .map(|timer| timer.next.saturating_duration_since(now))
            .min()
            .unwrap_or(maximum)
            .min(maximum)
    }

    fn due(&mut self, now: Instant) -> Vec<u32> {
        self.entries
            .iter_mut()
            .filter_map(|(&id, timer)| {
                if timer.next > now {
                    return None;
                }
                // Do not replay missed periods after a slow main-thread operation.
                timer.next = now + timer.period;
                Some(id)
            })
            .collect()
    }
}

pub(crate) fn wait(instance: &PluginInstance<EchodeHost>, maximum: Duration) -> Duration {
    instance.access_handler(|main| main.timers.borrow().wait(Instant::now(), maximum))
}

pub(crate) fn tick(instance: &PluginInstance<EchodeHost>) {
    let due = instance.access_handler(|main| main.timers.borrow_mut().due(Instant::now()));
    if due.is_empty() {
        return;
    }
    let plugin = instance.raw_instance();
    // SAFETY: this identifier names the official plugin timer-support table.
    let Some(timers) = (unsafe {
        crate::extension::table::<clap_plugin_timer_support>(plugin, CLAP_EXT_TIMER_SUPPORT)
    }) else {
        return;
    };
    let Some(on_timer) = timers.on_timer else {
        return;
    };
    for id in due {
        // Release every scheduler borrow before calling plugin code. A callback
        // can unregister itself or a later timer, or register another timer.
        if instance.access_handler(|main| main.timers.borrow().entries.contains_key(&id)) {
            // SAFETY: the instance is alive and this dispatch runs only on its
            // main thread. The ID remains registered after earlier callbacks.
            unsafe { on_timer(plugin, id) };
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HostTimers;
// SAFETY: this is CLAP's official host timer-support identifier.
unsafe impl Extension for HostTimers {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_TIMER_SUPPORT];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the table has the C layout and signatures declared by CLAP.
unsafe impl ExtensionImplementation<EchodeHost> for HostTimers {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_timer_support {
            register_timer: Some(register_timer),
            unregister_timer: Some(unregister_timer),
        });
}

unsafe extern "C" fn register_timer(host: *const clap_host, period_ms: u32, id: *mut u32) -> bool {
    // SAFETY: the plugin returns this instance's live host pointer. Check only
    // shared state before accessing the main-thread handler, even for bad calls.
    if id.is_null() || !unsafe { host_is_main_thread(host) } {
        return false;
    }
    // SAFETY: the thread check above permits access to the main-thread handler;
    // CLAP requires the non-null output pointer to be writable for one ID.
    unsafe {
        HostWrapper::<EchodeHost>::handle_main_thread(host, |main| {
            let Some(registered) = main.timers.borrow_mut().register(period_ms, Instant::now())
            else {
                return Ok(false);
            };
            id.write(registered);
            Ok(true)
        })
    }
    .unwrap_or(false)
}

unsafe extern "C" fn unregister_timer(host: *const clap_host, id: u32) -> bool {
    // SAFETY: as in registration, inspect shared state before main-thread state.
    if !unsafe { host_is_main_thread(host) } {
        return false;
    }
    // SAFETY: the host pointer is live and the calling thread was checked above.
    unsafe {
        HostWrapper::<EchodeHost>::handle_main_thread(host, |main| {
            Ok(main.timers.borrow_mut().unregister(id))
        })
    }
    .unwrap_or(false)
}

#[cfg(test)]
#[path = "plugin_timers_tests.rs"]
mod tests;
