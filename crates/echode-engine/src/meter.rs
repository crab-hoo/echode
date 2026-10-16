//! Read-only UI tap after the track fader. Only the audio owner publishes.
use std::sync::atomic::{AtomicU32, Ordering};

pub struct TrackMeter {
    pub track: u128,
    peak: AtomicU32,
}

impl TrackMeter {
    pub(crate) fn new(track: u128) -> Self {
        Self {
            track,
            peak: AtomicU32::new(0),
        }
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
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
        self.peak
            .store(peak.max(previous).to_bits(), Ordering::Relaxed);
    }
}
