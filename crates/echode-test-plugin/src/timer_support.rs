//! Main-thread timers, including cancellation and registration from a callback.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::timer_support::{
    CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support,
};
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::TestGain;

const FAILED: u8 = 0x80;
static STAGES: AtomicU8 = AtomicU8::new(0);

pub(crate) struct Probe {
    first: u32,
    cancelled: u32,
    periodic: u32,
    replacement: Option<u32>,
    periodic_calls: u8,
}

impl Probe {
    pub(crate) fn new(host: HostMainThreadHandle<'_>) -> Result<Self, PluginError> {
        let ext = host
            .get_extension::<HostTimers>()
            .ok_or(PluginError::Message(
                "timer-support extension is unavailable",
            ))?;
        // SAFETY: the extension belongs to this live host. Registration is on
        // the plugin main thread, with writable IDs or an explicitly null ID.
        let (first, cancelled, periodic) = unsafe {
            let table = ext.0.as_ptr().as_ref();
            let host = ext.0.host_ptr().as_ptr();
            let register = table
                .register_timer
                .ok_or(PluginError::Message("no register_timer"))?;
            let unregister = table
                .unregister_timer
                .ok_or(PluginError::Message("no unregister_timer"))?;
            let mut id = u32::MAX;
            if register(host, 0, &raw mut id)
                || id != u32::MAX
                || register(host, 1, std::ptr::null_mut())
                || unregister(host, u32::MAX)
            {
                return Err(PluginError::Message("invalid timer request was accepted"));
            }
            let mut ids = [u32::MAX; 3];
            for id in &mut ids {
                if !register(host, 1, id) || *id == u32::MAX {
                    return Err(PluginError::Message("timer registration failed"));
                }
            }
            let [first, cancelled, periodic] = ids;
            if first == cancelled || first == periodic || cancelled == periodic {
                return Err(PluginError::Message("timer IDs are not unique"));
            }
            (first, cancelled, periodic)
        };
        let shared = host.shared();
        let refused =
            std::thread::scope(|scope| scope.spawn(move || reject_off_main(shared).is_ok()).join())
                .map_err(|_| PluginError::Message("timer background probe panicked"))?;
        if !refused {
            return Err(PluginError::Message("timer background probe failed"));
        }
        STAGES.store(1, Ordering::Relaxed);
        Ok(Self {
            first,
            cancelled,
            periodic,
            replacement: None,
            periodic_calls: 0,
        })
    }

    fn on_timer(&mut self, host: HostMainThreadHandle<'_>, id: u32) -> Result<(), PluginError> {
        crate::thread_check::check(host.shared(), crate::thread_check::CALLBACK)?;
        let ext = host
            .get_extension::<HostTimers>()
            .ok_or(PluginError::Message("lost timer support"))?;
        // SAFETY: this callback runs on the main thread of the live host. No
        // plugin or host borrow is held across the host's scheduler dispatch.
        unsafe {
            let table = ext.0.as_ptr().as_ref();
            let raw_host = ext.0.host_ptr().as_ptr();
            let register = table
                .register_timer
                .ok_or(PluginError::Message("no register_timer"))?;
            let unregister = table
                .unregister_timer
                .ok_or(PluginError::Message("no unregister_timer"))?;
            if id == self.first {
                if !unregister(raw_host, self.first)
                    || !unregister(raw_host, self.cancelled)
                    || unregister(raw_host, self.first)
                {
                    return Err(PluginError::Message("timer cancellation failed"));
                }
                let mut replacement = u32::MAX;
                if !register(raw_host, 1, &raw mut replacement)
                    || [self.first, self.cancelled, self.periodic, u32::MAX].contains(&replacement)
                {
                    return Err(PluginError::Message(
                        "replacement timer registration failed",
                    ));
                }
                self.replacement = Some(replacement);
                STAGES.fetch_or(2, Ordering::Relaxed);
            } else if Some(id) == self.replacement {
                if !unregister(raw_host, id) {
                    return Err(PluginError::Message(
                        "replacement timer cancellation failed",
                    ));
                }
                STAGES.fetch_or(4, Ordering::Relaxed);
            } else if id == self.periodic {
                self.periodic_calls += 1;
                if self.periodic_calls > 2 {
                    return Err(PluginError::Message("cancelled periodic timer fired"));
                }
                if self.periodic_calls == 2 {
                    if !unregister(raw_host, id) {
                        return Err(PluginError::Message("periodic timer cancellation failed"));
                    }
                    STAGES.fetch_or(8, Ordering::Relaxed);
                }
            } else {
                return Err(PluginError::Message("cancelled or unknown timer fired"));
            }
        }
        Ok(())
    }
}

/// Deliberately invalid calls must not enter main-thread storage or allocate.
pub(crate) fn reject_off_main(host: HostSharedHandle<'_>) -> Result<(), PluginError> {
    let ext = host
        .get_extension::<HostTimers>()
        .ok_or(PluginError::Message(
            "timer-support extension is unavailable",
        ))?;
    // SAFETY: the host pointer and local output are valid. This probe tests the
    // host's refusal of an incorrect calling thread, without invalid memory.
    let refused = unsafe {
        let table = ext.0.as_ptr().as_ref();
        let host = ext.0.host_ptr().as_ptr();
        let mut id = u32::MAX;
        let registered = table
            .register_timer
            .is_some_and(|register| register(host, 1, &raw mut id));
        let removed = table
            .unregister_timer
            .is_some_and(|unregister| unregister(host, 0));
        !registered && !removed && id == u32::MAX
    };
    if refused {
        Ok(())
    } else {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        Err(PluginError::Message("off-main timer request was accepted"))
    }
}

pub(crate) fn snapshot() -> u8 {
    STAGES.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct HostTimers(RawExtension<HostExtensionSide, clap_host_timer_support>);
// SAFETY: the CLAP identifier selects the host timer-support table.
unsafe impl Extension for HostTimers {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_TIMER_SUPPORT];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier names this table's C layout.
        Self(unsafe { raw.cast() })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PluginTimers;
// SAFETY: the CLAP identifier selects the plugin timer-support table.
unsafe impl Extension for PluginTimers {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_TIMER_SUPPORT];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: the implementation uses the official table's C layout and signature.
unsafe impl ExtensionImplementation<TestGain> for PluginTimers {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_timer_support {
            on_timer: Some(on_timer),
        });
}

unsafe extern "C" fn on_timer(plugin: *const clap_plugin, id: u32) {
    // SAFETY: the host supplies this live plugin pointer on its main thread.
    let result = unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |plugin| {
            let main = plugin.main_thread();
            let host = main.host;
            Ok(main
                .timer_probe
                .borrow_mut()
                .as_mut()
                .ok_or(PluginError::Message("missing timer probe"))
                .and_then(|probe| probe.on_timer(host, id)))
        })
    };
    if !matches!(result, Some(Ok(()))) {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
    }
}
