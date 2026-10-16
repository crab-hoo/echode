//! Remove transport steps at the live device boundary. This is not applied to
//! offline rendering or ordinary note releases, and adds no lookahead latency.

/// Sample-counted live output transitions. Starts and location changes use an
/// offset join. Stops fade the continuing source waveform over 5 ms; the live
/// engine holds its public clock while producing only that bounded continuation.
pub struct OutputDeclick {
    last: [f32; 2],
    offset: [f32; 2],
    remaining: u32,
    frames: u32,
}

impl OutputDeclick {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            last: [0.0; 2],
            offset: [0.0; 2],
            remaining: 0,
            frames: (sample_rate / 200).max(2),
        }
    }

    /// Fade a continuing waveform, preserving its phase instead of replacing
    /// it with a held sample. The device supplies the absolute tail offset.
    pub fn process_stop(&mut self, left: &mut [f32], right: &mut [f32], offset: u32) {
        if left.is_empty() || left.len() != right.len() {
            return;
        }
        self.process(left, right, false);
        for (index, (left, right)) in left.iter_mut().zip(right).enumerate() {
            let age = u64::from(offset) + index as u64;
            let gain = if age >= u64::from(self.frames - 1) {
                0.0
            } else {
                0.5 * (1.0
                    + libm::cosf(std::f32::consts::PI * age as f32 / (self.frames - 1) as f32))
            };
            *left *= gain;
            *right *= gain;
            self.last = [*left, *right];
        }
        if u64::from(offset) + left.len() as u64 >= u64::from(self.frames) {
            self.offset = [0.0; 2];
            self.remaining = 0;
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], changed: bool) {
        if left.is_empty() || left.len() != right.len() {
            return;
        }
        if changed {
            self.offset = [self.last[0] - left[0], self.last[1] - right[0]];
            self.remaining = self.frames;
        }
        for (left, right) in left.iter_mut().zip(right) {
            if self.remaining > 0 {
                let gain = (self.remaining - 1) as f32 / (self.frames - 1) as f32;
                *left += self.offset[0] * gain;
                *right += self.offset[1] * gain;
                self.remaining -= 1;
            }
            self.last = [*left, *right];
        }
    }
}
