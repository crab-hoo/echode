//! The shared device-output fault predicate. Never limits or normalizes audio.

/// +18 dBFS, eight times full scale (ADR 0282).
pub const GUARD_DBFS: f32 = 18.0;
const GUARD: f32 = 7.943_282;

/// The first non-finite or runaway stereo frame, before any value is silenced.
pub(crate) fn first_fault(left: &[f32], right: &[f32]) -> Option<(usize, f32)> {
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(frame, (left, right))| {
            // `f32::max` drops a NaN, so inspect each channel first.
            let peak = [*left, *right]
                .into_iter()
                .find(|sample| sample.is_nan())
                .unwrap_or_else(|| left.abs().max(right.abs()));
            (peak.is_nan() || peak > GUARD).then_some((frame, peak))
        })
}

#[cfg(test)]
#[path = "output_guard_tests.rs"]
mod tests;
