//! When an output block must be finished within HAL's IO cycle (ADR 0283).
//!
//! HAL needs this block in the device ring buffer before the device reaches
//! the block's output time less its output safety offset. Other IOProcs on
//! the same device (the sample audition) and HAL's own write of the mix run
//! after this IOProc returns, inside that same interval.
use std::time::{Duration, Instant};

use echode_engine::block_duration;

/// Part of the block left for the rest of the IO cycle after this IOProc
/// returns: the audition IOProc on the same device and HAL's write before the
/// safety offset. It is a share of the block, like the engine's mix tail, not
/// a limit on tracks or plugins.
const CYCLE_RESERVE_DIVISOR: u32 = 16;

/// The prepared device's timing. Built on the control thread; the callback
/// only does arithmetic with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IoCycle {
    sample_rate: u32,
    safety_frames: u32,
    /// Mach timebase as (numer, denom), or `None` when it could not be read.
    timebase: Option<(u32, u32)>,
}

impl IoCycle {
    /// Reads the host clock's timebase. Control thread only.
    #[allow(deprecated)]
    pub(crate) fn new(sample_rate: u32, safety_frames: u32) -> Self {
        let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: the out parameter is correctly sized; this runs on control only.
        let read = unsafe { libc::mach_timebase_info(&mut timebase) } == libc::KERN_SUCCESS;
        Self::with_timebase(
            sample_rate,
            safety_frames,
            (read && timebase.numer != 0 && timebase.denom != 0)
                .then_some((timebase.numer, timebase.denom)),
        )
    }

    pub(crate) fn with_timebase(
        sample_rate: u32,
        safety_frames: u32,
        timebase: Option<(u32, u32)>,
    ) -> Self {
        Self {
            sample_rate,
            safety_frames,
            timebase,
        }
    }

    /// The absolute end of a callback entered at `entered`, when the host
    /// clock read `now_host`. `output_host` is HAL's output time for this
    /// block, or `None` when HAL marked it invalid.
    ///
    /// The end is never later than one block after entry, the bound used
    /// before the output time was known. An output time that is already
    /// inside the safety offset ends the callback at entry: waiting cannot
    /// make the block on time.
    pub(crate) fn deadline(
        &self,
        entered: Instant,
        now_host: u64,
        output_host: Option<u64>,
        frames: usize,
    ) -> Instant {
        let block = block_duration(frames, self.sample_rate);
        let (Some(output), Some((numer, denom))) = (output_host, self.timebase) else {
            return entered + block;
        };
        let ahead = host_duration(output.saturating_sub(now_host), numer, denom);
        let safety = block_duration(self.safety_frames as usize, self.sample_rate);
        let ready = ahead
            .saturating_sub(safety)
            .saturating_sub(block / CYCLE_RESERVE_DIVISOR);
        // Clamp before adding: a bogus output time far ahead must not
        // overflow `Instant`, and never extends the block.
        entered + ready.min(block)
    }
}

/// Host clock ticks as a duration, saturating instead of overflowing.
fn host_duration(ticks: u64, numer: u32, denom: u32) -> Duration {
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// The host clock now, in the units of HAL's time stamps.
#[allow(deprecated)]
pub(crate) fn host_now() -> u64 {
    // SAFETY: `mach_absolute_time` has no preconditions and does not block.
    unsafe { libc::mach_absolute_time() }
}

#[cfg(test)]
#[path = "io_cycle_tests.rs"]
mod tests;
