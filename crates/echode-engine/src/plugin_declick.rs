//! Sample-counted joins for missing plugin output, before the track fader.

/// Conceal a missing plugin block without changing healthy note onsets.
///
/// A miss fades the last emitted stereo sample to zero over 5 ms. Recovery
/// crossfades from the current emitted value to the new waveform over 5 ms.
/// Repeated misses do not restart the fade. No lookahead or allocation is used.
#[derive(Clone, Copy)]
pub struct PluginDeclick {
    last: [f32; 2],
    from: [f32; 2],
    remaining: u32,
    frames: u32,
    missed: bool,
}

impl PluginDeclick {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            last: [0.0; 2],
            from: [0.0; 2],
            remaining: 0,
            frames: (sample_rate / 200).max(2),
            missed: false,
        }
    }

    /// Forget the previous helper when a track no longer has a hosted chain.
    pub fn reset(&mut self) {
        self.last = [0.0; 2];
        self.from = [0.0; 2];
        self.remaining = 0;
        self.missed = false;
    }

    /// Process two equally sized output planes. Mono callers duplicate the plane.
    /// `missed` ignores the input; it never replays an abandoned helper block.
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], missed: bool) {
        if left.is_empty() || left.len() != right.len() {
            return;
        }
        if missed != self.missed {
            self.from = self.last;
            self.remaining = self.frames;
            self.missed = missed;
        }
        for (left, right) in left.iter_mut().zip(right) {
            let target = if missed {
                [0.0; 2]
            } else {
                [finite(*left), finite(*right)]
            };
            self.last = target;
            if self.remaining > 0 {
                let gain = f64::from(self.frames - self.remaining) / f64::from(self.frames - 1);
                for (value, from) in self.last.iter_mut().zip(self.from) {
                    // Convex interpolation in f64 also keeps finite f32 extremes finite.
                    *value = (f64::from(from) * (1.0 - gain) + f64::from(*value) * gain) as f32;
                }
                self.remaining -= 1;
            }
            [*left, *right] = self.last;
        }
    }
}

fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}
