//! Strict package fraction codec, separate from action argument decoding.
//! Keep package-specific unknown-field rejection without tightening the API.
use crate::Meter;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fraction {
    numerator: u32,
    denominator: u32,
}

pub(crate) mod meter {
    use super::*;
    pub fn serialize<S: Serializer>(meter: &Meter, serializer: S) -> Result<S::Ok, S::Error> {
        Fraction {
            numerator: meter.numerator(),
            denominator: meter.denominator(),
        }
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Meter, D::Error> {
        let parts = Fraction::deserialize(deserializer)?;
        Meter::try_new(parts.numerator, parts.denominator)
            .ok_or_else(|| D::Error::custom("meter does not divide into ticks"))
    }
}
