//! HAL overload callbacks may run synchronously on the device's IO thread.
//! Keep only bounded atomic work there; the owning control thread writes logs.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2_core_audio::*;

use crate::hardware::address;
use crate::io_cycle::host_now;

type Block = RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>;

#[derive(Default)]
struct Notices {
    count: AtomicU64,
    last_host_time: AtomicU64,
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn record(notices: &Notices) {
    notices.last_host_time.store(host_now(), Ordering::Relaxed);
    notices.count.fetch_add(1, Ordering::Release);
}

pub(crate) struct Listener {
    device: u32,
    source: &'static str,
    notices: Arc<Notices>,
    block: Block,
    registered: bool,
    reported: u64,
    next_log: Option<Instant>,
}

impl Listener {
    pub(crate) fn unregistered(device: u32, source: &'static str) -> Self {
        let notices = Arc::new(Notices::default());
        let captured = Arc::clone(&notices);
        let block = RcBlock::new(move |_: u32, _: NonNull<AudioObjectPropertyAddress>| {
            record(&captured);
        });
        Self {
            device,
            source,
            notices,
            block,
            registered: false,
            reported: 0,
            next_log: None,
        }
    }

    pub(crate) fn open(device: u32, source: &'static str) -> Option<Self> {
        let mut listener = Self::unregistered(device, source);
        let mut property = Self::property();
        // SAFETY: HAL copies the heap block, retaining only its Arc-owned atomics.
        // The property is valid for this call; Drop removes this exact registration.
        let status = unsafe {
            AudioObjectAddPropertyListenerBlock(
                device,
                NonNull::from(&mut property),
                None,
                NonNull::from(&*listener.block).as_ptr(),
            )
        };
        if status != 0 {
            log::warn!(
                "HAL overload monitoring unavailable: source={source} device={device} status={status}"
            );
            return None;
        }
        listener.registered = true;
        Some(listener)
    }

    fn property() -> AudioObjectPropertyAddress {
        address(
            kAudioDeviceProcessorOverload,
            kAudioObjectPropertyScopeGlobal,
        )
    }

    pub(crate) fn report(&mut self, sample: Option<i64>, callbacks: u64, force: bool) {
        self.report_at(sample, callbacks, force, Instant::now());
    }

    fn report_at(&mut self, sample: Option<i64>, callbacks: u64, force: bool, now: Instant) {
        let count = self.notices.count.load(Ordering::Acquire);
        let pending = count.wrapping_sub(self.reported);
        if pending == 0 || (!force && self.next_log.is_some_and(|next| now < next)) {
            return;
        }
        let host = self.notices.last_host_time.load(Ordering::Relaxed);
        // Position and callback ordinal are observations at drain time, not the
        // exact offending block. Concurrent notices may advance the host stamp.
        log::warn!(
            "HAL processor overload: source={} device={} notifications={} total={} last_notice_host_time={} observed_sample={sample:?} observed_callbacks={callbacks}",
            self.source,
            self.device,
            pending,
            count,
            host,
        );
        self.reported = count;
        self.next_log = Some(now + Duration::from_secs(1));
    }

    #[cfg(test)]
    pub(crate) fn notify(&self) {
        let mut property = Self::property();
        self.block.call((1, NonNull::from(&mut property)));
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.registered {
            let mut property = Self::property();
            // SAFETY: this is the same device, property, queue and block added
            // by open. A dispatched copy owns its atomics even after removal.
            let status = unsafe {
                AudioObjectRemovePropertyListenerBlock(
                    self.device,
                    NonNull::from(&mut property),
                    None,
                    NonNull::from(&*self.block).as_ptr(),
                )
            };
            if status != 0 {
                log::warn!(
                    "HAL overload listener removal failed: source={} device={} status={status}",
                    self.source,
                    self.device
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "overload_tests.rs"]
mod tests;
