//! Integer time arithmetic shared by the model, planner, and audio cursor.
//! Building an index allocates on control; querying it only searches and divides.

// The audio thread converts time with this crate (ADR 0284).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use std::sync::Arc;

/// Ticks in one quarter note: `2^6 * 3^2 * 5^2 * 7^2`.
pub const TICKS_PER_QUARTER: i64 = 705_600;
/// Flicks in one second.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;
const FLICKS_PER_TICK_AT_ONE_BPM: i128 = 60_000;

/// Nearest integer, with exact halves rounded away from zero.
pub fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    if denominator <= 0 {
        return None;
    }
    let half = denominator / 2;
    let adjusted = if numerator >= 0 {
        numerator.checked_add(half)?
    } else {
        numerator.checked_sub(half)?
    };
    Some(adjusted / denominator)
}

pub fn flicks_for_ticks(ticks: i128, numerator: u32, denominator: u32) -> Option<i128> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    let scale = FLICKS_PER_TICK_AT_ONE_BPM * i128::from(denominator);
    div_round(ticks.checked_mul(scale)?, i128::from(numerator))
}

pub fn ticks_for_flicks(flicks: i128, numerator: u32, denominator: u32) -> Option<i128> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    let scale = FLICKS_PER_TICK_AT_ONE_BPM * i128::from(denominator);
    div_round(flicks.checked_mul(i128::from(numerator))?, scale)
}

pub fn flick_to_sample(flick: i128, sample_rate: u32) -> Option<i64> {
    if sample_rate == 0 {
        return None;
    }
    let samples = div_round(
        flick.checked_mul(i128::from(sample_rate))?,
        i128::from(FLICKS_PER_SECOND),
    )?;
    i64::try_from(samples).ok()
}

pub fn sample_to_flick(sample: i64, sample_rate: u32) -> Option<i128> {
    div_round(
        i128::from(sample) * i128::from(FLICKS_PER_SECOND),
        i128::from(sample_rate),
    )
}

/// Floor a device block's duration in Mach timebase units, before caller bounds.
pub fn block_period_ticks(frames: u32, rate: u128, numer: u32, denom: u32) -> Option<u128> {
    if frames == 0 || rate == 0 || numer == 0 || denom == 0 {
        return None;
    }
    let divisor = rate.checked_mul(u128::from(numer))?;
    Some(u128::from(frames) * 1_000_000_000 * u128::from(denom) / divisor)
}

/// A tempo boundary and its cumulative, per-segment rounded flick position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoSegment {
    pub start_tick: i64,
    pub start_flick: i128,
    pub numerator: u32,
    pub denominator: u32,
}

/// Shared immutable boundaries. Clone is constant-time; queries are O(log T).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoIndex {
    segments: Arc<[TempoSegment]>,
}

fn allowed(numerator: u32, denominator: u32) -> bool {
    numerator > 0 && denominator > 0 && u64::from(numerator) <= 999 * u64::from(denominator)
}

impl TempoIndex {
    /// Accepts nonempty, strictly ordered, pre-integrated boundaries.
    pub fn new(segments: Vec<TempoSegment>) -> Option<Self> {
        if segments.is_empty()
            || segments
                .iter()
                .any(|s| !allowed(s.numerator, s.denominator))
            || segments.windows(2).any(|pair| {
                matches!(pair, [left, right] if left.start_tick >= right.start_tick
                    || left.start_flick >= right.start_flick)
            })
        {
            return None;
        }
        Some(Self {
            segments: segments.into(),
        })
    }

    /// Builds in one pass per direction from sorted `(tick, numerator, denominator)`.
    /// The first tempo extends before its point. Zero is always the time origin,
    /// even when the input has no point there or has points before it.
    pub fn from_tempos(points: impl IntoIterator<Item = (i64, u32, u32)>) -> Option<Self> {
        let mut segments: Vec<_> = points
            .into_iter()
            .map(|(start_tick, numerator, denominator)| TempoSegment {
                start_tick,
                start_flick: 0,
                numerator,
                denominator,
            })
            .collect();
        if segments.is_empty()
            || segments
                .iter()
                .any(|s| !allowed(s.numerator, s.denominator))
            || segments
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left.start_tick >= right.start_tick))
        {
            return None;
        }
        let zero = segments.partition_point(|segment| segment.start_tick < 0);
        if segments
            .get(zero)
            .is_none_or(|segment| segment.start_tick != 0)
        {
            let tempo = *segments.get(zero.saturating_sub(1))?;
            segments.insert(
                zero,
                TempoSegment {
                    start_tick: 0,
                    ..tempo
                },
            );
        }
        for index in zero..segments.len().saturating_sub(1) {
            let [start, end] = segments.get_mut(index..index + 2)? else {
                return None;
            };
            end.start_flick = start.start_flick.checked_add(flicks_for_ticks(
                i128::from(end.start_tick) - i128::from(start.start_tick),
                start.numerator,
                start.denominator,
            )?)?;
        }
        for index in (0..zero).rev() {
            let [start, end] = segments.get_mut(index..index + 2)? else {
                return None;
            };
            start.start_flick = end.start_flick.checked_sub(flicks_for_ticks(
                i128::from(end.start_tick) - i128::from(start.start_tick),
                start.numerator,
                start.denominator,
            )?)?;
        }
        Self::new(segments)
    }

    pub fn tick_to_flick(&self, tick: i64) -> Option<i128> {
        let after = self.segments.partition_point(|segment| {
            visit_boundary();
            segment.start_tick <= tick
        });
        let segment = self.segments.get(after.saturating_sub(1))?;
        // The model integrates [tick, 0] backwards, rounding each segment before
        // negating. A negative partial interval must use its right boundary:
        // round(a + b) - round(a) is not always round(b).
        if tick < 0
            && let Some(end) = self.segments.get(after)
            && end.start_tick <= 0
        {
            return end.start_flick.checked_sub(flicks_for_ticks(
                i128::from(end.start_tick) - i128::from(tick),
                segment.numerator,
                segment.denominator,
            )?);
        }
        segment.start_flick.checked_add(flicks_for_ticks(
            i128::from(tick) - i128::from(segment.start_tick),
            segment.numerator,
            segment.denominator,
        )?)
    }

    pub fn flick_to_tick(&self, flick: i128) -> Option<i64> {
        let index = self
            .segments
            .partition_point(|segment| {
                visit_boundary();
                segment.start_flick <= flick
            })
            .saturating_sub(1);
        let segment = self.segments.get(index)?;
        let ticks = ticks_for_flicks(
            flick.checked_sub(segment.start_flick)?,
            segment.numerator,
            segment.denominator,
        )?;
        i64::try_from(i128::from(segment.start_tick).checked_add(ticks)?).ok()
    }

    pub fn tick_to_sample(&self, tick: i64, sample_rate: u32) -> Option<i64> {
        flick_to_sample(self.tick_to_flick(tick)?, sample_rate)
    }

    pub fn sample_to_tick(&self, sample: i64, sample_rate: u32) -> Option<i64> {
        self.flick_to_tick(sample_to_flick(sample, sample_rate)?)
    }
}

#[inline(always)]
fn visit_boundary() {
    #[cfg(test)]
    VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(test)]
std::thread_local! {
    static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
