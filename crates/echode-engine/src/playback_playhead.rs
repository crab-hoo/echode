//! Source trajectory for the display and compensated per-stage musical clocks.
use crate::PlaybackError;

/// One continuous source trajectory. Jumps and loop-range changes start a new
/// trajectory; the display holds its origin until that source reaches master.
/// Counting device frames, rather than subtracting from the wrapped clock,
/// also handles the first loop entry and delays longer than a loop.
#[derive(Clone, Copy)]
pub(crate) struct Playhead {
    origin: i64,
    frames: u64,
    /// The last successfully rendered source block, not a newly prepared plan.
    latency: u64,
    loop_bounds: Option<(i64, i64)>,
}

impl Playhead {
    pub(crate) fn new(origin: i64, bounds: (Option<i64>, Option<i64>)) -> Self {
        Self {
            origin,
            frames: 0,
            latency: 0,
            loop_bounds: bounds
                .0
                .zip(bounds.1)
                .filter(|&(start, end)| start < end && origin < end),
        }
    }

    pub(crate) fn advance(&mut self, frames: usize, latency: u64) -> Result<(), PlaybackError> {
        self.frames = self
            .frames
            .checked_add(frames as u64)
            .ok_or(PlaybackError::Time)?;
        self.latency = latency;
        Ok(())
    }

    /// Input clocks extrapolate before the origin while their delay line fills;
    /// only the display holds the origin during this pre-roll.
    pub(crate) fn input_sample(&self, latency: u64) -> Result<i64, PlaybackError> {
        if let Some(pending) = latency.checked_sub(self.frames) {
            return i64::try_from(i128::from(self.origin) - i128::from(pending))
                .map_err(|_| PlaybackError::Time);
        }
        Ok(Self { latency, ..*self }.sample())
    }

    /// Frames before this trajectory's source reaches an input.
    pub(crate) fn note_offset(&self, latency: u64, frames: usize) -> usize {
        latency.saturating_sub(self.frames).min(frames as u64) as usize
    }

    pub(crate) fn note_origin(&self) -> i64 {
        self.origin
    }
    /// Preserve an exact loop-end landing for note release before wrapping.
    pub(crate) fn note_sample(&self, latency: u64) -> Result<i64, PlaybackError> {
        let sample = self.input_sample(latency)?;
        if let Some((start, end)) = self.loop_bounds {
            let elapsed = i128::from(self.frames) - i128::from(latency);
            if sample == start && elapsed >= i128::from(end) - i128::from(self.origin) {
                return Ok(end);
            }
        }
        Ok(sample)
    }
    pub(crate) fn loop_bounds(&self) -> (Option<i64>, Option<i64>) {
        (
            self.loop_bounds.map(|bounds| bounds.0),
            self.loop_bounds.map(|bounds| bounds.1),
        )
    }

    pub(super) fn sample(&self) -> i64 {
        let elapsed = i128::from(self.frames.saturating_sub(self.latency));
        let position = if let Some((start, end)) = self.loop_bounds {
            let until_end = i128::from(end) - i128::from(self.origin);
            if elapsed >= until_end {
                i128::from(start) + (elapsed - until_end) % (i128::from(end) - i128::from(start))
            } else {
                i128::from(self.origin) + elapsed
            }
        } else {
            i128::from(self.origin) + elapsed
        };
        // A running non-looping render clock rejects overflow before publishing.
        i64::try_from(position).unwrap_or(i64::MAX)
    }
}
