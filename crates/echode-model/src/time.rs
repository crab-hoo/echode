//! Integer musical time and real time.
//!
//! One quarter note is [`TICKS_PER_QUARTER`] ticks. One second is
//! [`FLICKS_PER_SECOND`] flicks. Both divide the sample rates and tuplets the
//! sequencer needs, so a rational-tempo conversion rounds at most half a flick
//! per constant-tempo segment.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

pub use echode_time::{FLICKS_PER_SECOND, TICKS_PER_QUARTER};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

macro_rules! define_tempo_point {
    ($($property:ident => $field:ident: $type:ty, $value:ident;
        package { $($wire:ident: $wire_type:ty = $encode:expr;)+ }
        restore $decode:expr;)+) => {
        /// One step of a tempo map, addressed by its tick.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(try_from = "TempoPointParts", into = "TempoPointParts")]
        pub struct TempoPoint {
            pub at: Tick,
            $(pub $field: $type,)+
        }
        // The package flattens the tempo fraction next to the identity tick.
        #[derive(Clone, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct TempoPointParts { at: Tick, $($($wire: $wire_type,)+)+ }
        impl From<TempoPoint> for TempoPointParts {
            fn from(point: TempoPoint) -> Self {
                let TempoPoint { at, $($field,)+ } = point;
                Self { at, $($($wire: $encode,)+)+ }
            }
        }
        impl TryFrom<TempoPointParts> for TempoPoint {
            type Error = &'static str;
            fn try_from(parts: TempoPointParts) -> Result<Self, Self::Error> {
                let TempoPointParts { at, $($($wire,)+)+ } = parts;
                Ok(Self { at, $($field: ($decode)?,)+ })
            }
        }
    };
}
crate::tempo_fields::tempo_fields!(define_tempo_point);

/// Constant tempo between points. Tick zero is flick zero.
///
/// The first point also covers every tick before it. A linear ramp is not
/// represented yet: a ramp needs its own closed form, and Phase 1 playback
/// starts from constant sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoMap {
    points: Arc<[TempoPoint]>,
    index: echode_time::TempoIndex,
}

impl TempoMap {
    pub fn new(tempo: Tempo) -> Self {
        // A single validated tempo at zero always forms a valid index.
        Self::try_from_points(vec![TempoPoint {
            at: Tick::new(0),
            tempo,
        }])
        .expect("a validated tempo makes a valid index")
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
        let index = echode_time::TempoIndex::from_tempos(points.iter().map(|point| {
            (
                point.at.get(),
                point.tempo.numerator(),
                point.tempo.denominator(),
            )
        }))?;
        Some(Self {
            points: points.into(),
            index,
        })
    }

    /// Inserts or replaces the tempo that starts at `at`.
    #[must_use]
    pub fn with_tempo(&self, at: Tick, tempo: Tempo) -> Self {
        let mut points = self.points.to_vec();
        match points.binary_search_by_key(&at, |point| point.at) {
            Ok(index) => points[index].tempo = tempo,
            Err(index) => points.insert(index, TempoPoint { at, tempo }),
        }
        Self::try_from_points(points).expect("editing a validated tempo preserves the index")
    }

    /// Real time of a musical position.
    pub fn flicks_at(&self, tick: Tick) -> Option<Flick> {
        i64::try_from(self.index.tick_to_flick(tick.0)?)
            .ok()
            .map(Flick)
    }

    /// Musical position of a real time. Exact when the forward conversion was exact.
    pub fn ticks_at(&self, flick: Flick) -> Option<Tick> {
        self.index.flick_to_tick(i128::from(flick.0)).map(Tick)
    }

    /// Shared conversion index, also used by playback without rebuilding it.
    pub fn index(&self) -> &echode_time::TempoIndex {
        &self.index
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
    echode_time::flick_to_sample(i128::from(tempo.flicks_at(tick)?.get()), sample_rate)
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
