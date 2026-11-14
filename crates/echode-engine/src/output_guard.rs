//! The shared fault predicate for planes the engine narrows or hands on.
//! Never limits or normalizes audio.

/// +18 dBFS, eight times full scale (ADR 0282).
pub const GUARD_DBFS: f32 = 18.0;
const GUARD: f32 = 7.943_282;

/// The first non-finite or runaway stereo frame, before any value is silenced.
/// The device outputs' rule: the master and the sample audition (ADR 0282, ADR 0344).
pub(crate) fn first_fault(left: &[f32], right: &[f32]) -> Option<(usize, f32)> {
    first_past(left, right, GUARD)
}

/// The first frame holding a NaN or an infinity: where an `f64` sum did not
/// fit the `f32` plane it was narrowed into (NFA-01 to NFA-03). The rule of an export's
/// master and of every folder bus, which have no level limit.
pub(crate) fn first_nonfinite(left: &[f32], right: &[f32]) -> Option<(usize, f32)> {
    // Only an infinity is greater than `f32::MAX`.
    first_past(left, right, f32::MAX)
}

/// The first frame with a NaN, or with a magnitude greater than `limit`, and
/// that NaN or magnitude.
fn first_past(left: &[f32], right: &[f32], limit: f32) -> Option<(usize, f32)> {
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(frame, (left, right))| {
            // `f32::max` drops a NaN, so inspect each channel first.
            let peak = [*left, *right]
                .into_iter()
                .find(|sample| sample.is_nan())
                .unwrap_or_else(|| left.abs().max(right.abs()));
            (peak.is_nan() || peak > limit).then_some((frame, peak))
        })
}

#[cfg(test)]
#[path = "output_guard_tests.rs"]
mod tests;
