//! Integer musical time and real time.
//!
//! One quarter note is [`TICKS_PER_QUARTER`] ticks. One second is
//! [`FLICKS_PER_SECOND`] flicks. Both divide the sample rates and tuplets the
//! sequencer needs, so a conversion at a rational tempo rounds at most half a
//! flick for the whole span.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Ticks in one quarter note. `2^6 * 3^2 * 5^2 * 7^2`.
pub const TICKS_PER_QUARTER: i64 = 705_600;

/// Flicks in one second.
pub const FLICKS_PER_SECOND: i64 = 705_600_000;

/// `FLICKS_PER_SECOND * 60 / TICKS_PER_QUARTER`.
///
/// A span of `ticks` at `n/d` quarters per minute lasts
/// `ticks * 60_000 * d / n` flicks.
const FLICKS_PER_TICK_AT_ONE_BPM: i128 = 60_000;

/// Musical position. Zero is the project origin. Negative values are before it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(i64);

/// Real time. Zero matches musical tick zero.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Flick(i64);

impl Tick {
    pub const fn new(ticks: i64) -> Self {
        Self(ticks)
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    /// Whole quarter notes, or `None` when the multiplication overflows.
    pub fn quarters(count: i64) -> Option<Self> {
        count.checked_mul(TICKS_PER_QUARTER).map(Self)
    }
}

impl Flick {
    pub const fn new(flicks: i64) -> Self {
        Self(flicks)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Debug for Tick {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Tick({})", self.0)
    }
}

impl fmt::Debug for Flick {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Flick({})", self.0)
    }
}

/// Quarter notes per minute, as a reduced fraction no faster than 999.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tempo {
    numerator: u32,
    denominator: u32,
}

impl Tempo {
    pub const fn quarters_per_minute(bpm: u32) -> Option<Self> {
        if bpm == 0 || bpm > 999 {
            None
        } else {
            Some(Self {
                numerator: bpm,
                denominator: 1,
            })
        }
    }

    /// `numerator/denominator` quarter notes per minute.
    pub fn rational(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        if u64::from(numerator) > 999 * u64::from(denominator) {
            return None;
        }
        let divisor = gcd(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

/// One step of a tempo map. The tempo holds until the next point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoPoint {
    pub at: Tick,
    pub tempo: Tempo,
}

/// Constant tempo between points. Tick zero is flick zero.
///
/// The first point also covers every tick before it. A linear ramp is not
/// represented yet: a ramp needs its own closed form, and Phase 1 playback
/// starts from constant sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoMap {
    points: Vec<TempoPoint>,
}

impl TempoMap {
    pub fn new(tempo: Tempo) -> Self {
        Self {
            points: vec![TempoPoint {
                at: Tick::new(0),
                tempo,
            }],
        }
    }

    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    /// Stores points in tick order. An empty list, or two points at the same tick, is refused.
    pub fn try_from_points(mut points: Vec<TempoPoint>) -> Option<Self> {
        if points.is_empty() {
            return None;
        }
        points.sort_by_key(|point| point.at);
        if points.windows(2).any(|pair| pair[0].at == pair[1].at) {
            return None;
        }
        Some(Self { points })
    }

    /// Inserts or replaces the tempo that starts at `at`.
    #[must_use]
    pub fn with_tempo(&self, at: Tick, tempo: Tempo) -> Self {
        let mut points = self.points.clone();
        if let Some(index) = points.iter().position(|point| point.at == at) {
            points[index].tempo = tempo;
        } else {
            points.push(TempoPoint { at, tempo });
            points.sort_by_key(|point| point.at);
        }
        Self { points }
    }

    /// Real time of a musical position.
    pub fn flicks_at(&self, tick: Tick) -> Option<Flick> {
        let flicks = if tick.0 >= 0 {
            integrate(self, 0, tick.0)?
        } else {
            -integrate(self, tick.0, 0)?
        };
        i64::try_from(flicks).ok().map(Flick)
    }

    /// Musical position of a real time. Exact when the forward conversion was exact.
    pub fn ticks_at(&self, flick: Flick) -> Option<Tick> {
        let tick = if flick.0 >= 0 {
            invert_forward(self, flick.0)?
        } else {
            invert_backward(self, flick.0)?
        };
        Some(Tick(tick))
    }
}

/// Time signature. A bar is `numerator * 4 / denominator` quarter notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Meter {
    numerator: u32,
    denominator: u32,
}

impl Meter {
    pub const fn four_four() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }

    /// Accepts a signature whose bar length is a whole number of ticks.
    pub fn try_new(numerator: u32, denominator: u32) -> Option<Self> {
        let meter = Self {
            numerator,
            denominator,
        };
        meter.bar_ticks()?;
        Some(meter)
    }

    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    pub fn bar_ticks(self) -> Option<Tick> {
        if self.denominator == 0 || self.numerator == 0 {
            return None;
        }
        let numerator = i128::from(self.numerator) * i128::from(TICKS_PER_QUARTER) * 4;
        let denominator = i128::from(self.denominator);
        if numerator % denominator != 0 {
            return None;
        }
        let ticks = numerator / denominator;
        i64::try_from(ticks)
            .ok()
            .filter(|ticks| *ticks > 0)
            .map(Tick)
    }
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
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

fn flicks_for_ticks(ticks: i128, tempo: Tempo) -> Option<i128> {
    let scale = FLICKS_PER_TICK_AT_ONE_BPM.checked_mul(i128::from(tempo.denominator))?;
    div_round(ticks.checked_mul(scale)?, i128::from(tempo.numerator))
}

fn ticks_for_flicks(flicks: i128, tempo: Tempo) -> Option<i128> {
    let scale = FLICKS_PER_TICK_AT_ONE_BPM.checked_mul(i128::from(tempo.denominator))?;
    div_round(flicks.checked_mul(i128::from(tempo.numerator))?, scale)
}

fn tempo_covering(map: &TempoMap, tick: i64) -> Tempo {
    let mut tempo = map.points[0].tempo;
    for point in &map.points {
        if point.at.0 <= tick {
            tempo = point.tempo;
        } else {
            break;
        }
    }
    tempo
}

fn next_boundary(map: &TempoMap, tick: i64) -> Option<i64> {
    map.points
        .iter()
        .map(|point| point.at.0)
        .find(|&at| at > tick)
}

fn previous_boundary(map: &TempoMap, tick: i64) -> Option<i64> {
    map.points
        .iter()
        .map(|point| point.at.0)
        .rfind(|&at| at < tick)
}

/// Flicks from `from` to `to`, with `from <= to`.
fn integrate(map: &TempoMap, from: i64, to: i64) -> Option<i128> {
    let mut cursor = from;
    let mut accumulated = 0i128;
    while cursor < to {
        let tempo = tempo_covering(map, cursor);
        let end = next_boundary(map, cursor).unwrap_or(to).min(to);
        let span = i128::from(end) - i128::from(cursor);
        accumulated = accumulated.checked_add(flicks_for_ticks(span, tempo)?)?;
        cursor = end;
    }
    Some(accumulated)
}

fn add_ticks(tick: i64, delta: i128) -> Option<i64> {
    i64::try_from(i128::from(tick).checked_add(delta)?).ok()
}

fn invert_forward(map: &TempoMap, target: i64) -> Option<i64> {
    let target = i128::from(target);
    let mut tick = 0i64;
    let mut flick = 0i128;
    loop {
        let tempo = tempo_covering(map, tick);
        let remain = target - flick;
        if let Some(end) = next_boundary(map, tick) {
            let span = i128::from(end) - i128::from(tick);
            let segment = flicks_for_ticks(span, tempo)?;
            if flick + segment >= target {
                return add_ticks(tick, ticks_for_flicks(remain, tempo)?);
            }
            flick += segment;
            tick = end;
        } else {
            return add_ticks(tick, ticks_for_flicks(remain, tempo)?);
        }
    }
}

fn invert_backward(map: &TempoMap, target: i64) -> Option<i64> {
    let target = i128::from(target);
    let mut tick_end = 0i64;
    let mut flick_end = 0i128;
    loop {
        let tempo = tempo_covering(map, tick_end.checked_sub(1)?);
        if let Some(start) = previous_boundary(map, tick_end) {
            let span = i128::from(tick_end) - i128::from(start);
            let segment = flicks_for_ticks(span, tempo)?;
            let flick_start = flick_end - segment;
            if flick_start <= target {
                let into = target - flick_start;
                return add_ticks(start, ticks_for_flicks(into, tempo)?);
            }
            flick_end = flick_start;
            tick_end = start;
        } else {
            let delta = flick_end - target;
            return add_ticks(tick_end, -ticks_for_flicks(delta, tempo)?);
        }
    }
}

impl Serialize for Tick {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Tick {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        i64::deserialize(deserializer).map(Self)
    }
}

impl JsonSchema for Tick {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Tick".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "integer" })
    }
}

impl Serialize for Meter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Meter", 2)?;
        state.serialize_field("numerator", &self.numerator)?;
        state.serialize_field("denominator", &self.denominator)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Meter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Parts {
            numerator: u32,
            denominator: u32,
        }
        let parts = Parts::deserialize(deserializer)?;
        Self::try_new(parts.numerator, parts.denominator)
            .ok_or_else(|| D::Error::custom("meter does not divide into ticks"))
    }
}

impl JsonSchema for Meter {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Meter".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "numerator": { "type": "integer" },
                "denominator": { "type": "integer" }
            },
            "required": ["numerator", "denominator"]
        })
    }
}

impl Serialize for Tempo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Tempo", 2)?;
        state.serialize_field("numerator", &self.numerator)?;
        state.serialize_field("denominator", &self.denominator)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Tempo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Parts {
            numerator: u32,
            denominator: u32,
        }
        let parts = Parts::deserialize(deserializer)?;
        Self::rational(parts.numerator, parts.denominator)
            .ok_or_else(|| D::Error::custom("tempo is outside the allowed range"))
    }
}

impl JsonSchema for Tempo {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Tempo".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "numerator": { "type": "integer" },
                "denominator": { "type": "integer" }
            },
            "required": ["numerator", "denominator"]
        })
    }
}

/// Sample index of `tick` at `sample_rate`, rounded once from the flick position.
pub fn tick_to_sample(tick: Tick, tempo: &TempoMap, sample_rate: u32) -> Option<i64> {
    if sample_rate == 0 {
        return None;
    }
    let flicks = tempo.flicks_at(tick)?;
    let samples = div_round(
        i128::from(flicks.get()) * i128::from(sample_rate),
        i128::from(FLICKS_PER_SECOND),
    )?;
    i64::try_from(samples).ok()
}

/// Sample offset of timeline `tick` from content position 0 at `origin`.
///
/// Negative means `tick` is before the media file. The tempo integral is
/// `[origin, tick]`, which is not the same length as a later interval of the
/// same tick width when the tempo changes.
pub fn media_sample_at(origin: i64, tick: i64, tempo: &TempoMap, sample_rate: u32) -> Option<i64> {
    let at = tick_to_sample(Tick::new(tick), tempo, sample_rate)?;
    let origin_at = tick_to_sample(Tick::new(origin), tempo, sample_rate)?;
    at.checked_sub(origin_at)
}

/// Samples from media position 0 to the placement start.
///
/// Content position 0 is anchored at `start - offset`. A positive lead skips
/// that many samples into the file. A negative lead is silence before the file.
pub fn placement_media_lead(
    start: i64,
    offset: i64,
    tempo: &TempoMap,
    sample_rate: u32,
) -> Option<i64> {
    let origin = start.checked_sub(offset)?;
    media_sample_at(origin, start, tempo, sample_rate)
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
