//! Read-only UI tap after the track fader. Only the audio owner publishes.
use std::sync::atomic::{AtomicU32, Ordering};

/// Linear amplitude at -60 dB. Header meters, the inspector readout, and the
/// display-link hold all treat this as silence (`MixScale.meterSilence`).
const DISPLAY_FLOOR: f32 = 0.001;

pub struct TrackMeter {
    pub track: u128,
    peak: AtomicU32,
    #[cfg(test)]
    publications: AtomicU32,
}

impl TrackMeter {
    pub(crate) fn new(track: u128) -> Self {
        Self {
            track,
            peak: AtomicU32::new(0),
            #[cfg(test)]
            publications: AtomicU32::new(0),
        }
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    /// Drop a peak that never reached the device. The release tail does not apply.
    pub(crate) fn clear(&self) {
        self.peak.store(0f32.to_bits(), Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn publications(&self) -> u32 {
        self.publications.load(Ordering::Relaxed)
    }

    pub(crate) fn release(frames: usize, rate: u32) -> f32 {
        // Instant attack and a 24 dB/s release retain short transients between
        // display frames. The audio sample count also drives decay while paused.
        if frames == 0 {
            1.0
        } else {
            libm::expf(-std::f32::consts::LN_10 * 1.2 * frames as f32 / rate as f32)
        }
    }

    pub(crate) fn publish(&self, peak: f32, release: f32) {
        let previous = self.peak() * release;
        let peak = if peak.is_finite() {
            peak.max(0.0)
        } else {
            f32::MAX
        };
        let held = peak.max(previous);
        // Below the shared -60 dB floor the bar is already empty. Keeping the
        // tail makes the dB readout run away and feeds denormals back in here.
        let held = if held <= DISPLAY_FLOOR { 0.0 } else { held };
        self.peak.store(held.to_bits(), Ordering::Relaxed);
        #[cfg(test)]
        {
            self.publications.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TrackMeter;

    #[test]
    fn release_is_twenty_four_db_per_second_and_silence_is_zero() {
        let meter = TrackMeter::new(1);
        meter.publish(1.0, 1.0);
        assert_eq!(meter.peak(), 1.0);
        let one_second = TrackMeter::release(48_000, 48_000);
        meter.publish(0.0, one_second);
        let after_one = meter.peak();
        let db = 20.0 * f64::from(after_one).log10();
        assert!((db + 24.0).abs() < 0.05, "expected about -24 dB, got {db}");
        // The same elapsed time in small blocks stays on that curve.
        let chunked = TrackMeter::new(1);
        chunked.publish(1.0, 1.0);
        let chunk = TrackMeter::release(64, 48_000);
        for _ in 0..(48_000 / 64) {
            chunked.publish(0.0, chunk);
        }
        assert!((chunked.peak() - after_one).abs() < 1.0e-4);

        for _ in 0..3 {
            meter.publish(0.0, one_second);
            chunked.publish(0.0, one_second);
        }
        assert_eq!(meter.peak(), 0.0);
        assert_eq!(chunked.peak(), 0.0);
        meter.publish(0.0, one_second);
        assert_eq!(meter.peak(), 0.0, "silence kept decaying");
        meter.publish(0.25, 1.0);
        assert_eq!(meter.peak(), 0.25);
    }
}
