//! One registry for change value types and their legacy journal codecs.
//! These are wire values, not proof that a project satisfies its invariants.
use crate::{AudioRef, ClipKind, Id, Meter, ModelError, Tempo, Tick};
use serde::{Deserialize, Serialize};

macro_rules! define_values {
    ($($variant:ident($binding:ident: $type:ty) {
        $($(#[$attribute:meta])* $field:ident: $wire:ty = $encode:expr;)+
    } => $decode:expr;)+) => {
        /// A property value. The variant must match its property and object kind.
        #[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
        #[serde(try_from = "ValueRecord")]
        pub enum Value {
            $($variant($type),)+
            /// One insertion/removal, rather than two full note ID lists.
            OrderStep { index: u32, id: Id, present: bool },
        }

        #[derive(Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum ValueRecord {
            $($variant { $($(#[$attribute])* $field: $wire,)+ },)+
            OrderStep { index: u32, id: String, present: bool },
        }
        impl Serialize for Value {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                ValueRecord::from(self).serialize(serializer)
            }
        }
        impl From<&Value> for ValueRecord {
            fn from(value: &Value) -> Self {
                match value {
                    $(Value::$variant($binding) => Self::$variant { $($field: $encode,)+ },)+
                    Value::OrderStep { index, id, present } => Self::OrderStep {
                        index: *index, id: id.to_string(), present: *present,
                    },
                }
            }
        }
        impl TryFrom<ValueRecord> for Value {
            type Error = ModelError;
            fn try_from(record: ValueRecord) -> Result<Self, Self::Error> {
                Ok(match &record {
                    $(ValueRecord::$variant { $($field,)+ } => Value::$variant(($decode)?),)+
                    ValueRecord::OrderStep { index, id, present } => Value::OrderStep {
                        index: *index, id: parse_id(id)?, present: *present,
                    },
                })
            }
        }
    };
}

define_values! {
    Meter(meter: Meter) {
        numerator: u32 = meter.numerator();
        denominator: u32 = meter.denominator();
    } => Meter::try_new(*numerator, *denominator).ok_or(ModelError::InvalidChange);
    Tempo(tempo: Tempo) {
        numerator: u32 = tempo.numerator();
        denominator: u32 = tempo.denominator();
    } => Tempo::rational(*numerator, *denominator).ok_or(ModelError::InvalidChange);
    Tick(tick_value: Tick) { tick: i64 = tick_value.get(); } => Ok::<_, ModelError>(Tick::new(*tick));
    Text(text_value: String) { text: String = text_value.clone(); } => Ok::<_, ModelError>(text.clone());
    Id(id_value: Id) { id: String = id_value.to_string(); } => parse_id(id);
    Ids(ids_value: Vec<Id>) {
        ids: Vec<String> = ids_value.iter().map(ToString::to_string).collect();
    } => ids.iter().map(|id| parse_id(id)).collect::<Result<Vec<_>, _>>();
    Parent(parent: Option<Id>) {
        id: Option<String> = parent.map(|id| id.to_string());
    } => id.as_deref().map(parse_id).transpose();
    Audio(audio: Option<AudioRef>) {
        hash: Option<String> = audio.as_ref().map(|audio| audio.hash.clone());
        #[serde(default)]
        sample_rate: u32 = audio.as_ref().map(|audio| audio.sample_rate).unwrap_or(0);
        #[serde(default)]
        channels: u16 = audio.as_ref().map(|audio| audio.channels).unwrap_or(0);
        #[serde(default)]
        frames: u64 = audio.as_ref().map(|audio| audio.frames).unwrap_or(0);
    } => hash.as_ref().map(|hash| AudioRef::try_new(hash.clone(), *sample_rate, *channels, *frames)).transpose();
    Kind(kind_value: ClipKind) {
        kind: String = match kind_value { ClipKind::Auto => "auto", ClipKind::Fixed => "fixed" }.into();
    } => match kind.as_str() {
        "auto" => Ok(ClipKind::Auto), "fixed" => Ok(ClipKind::Fixed), _ => Err(ModelError::InvalidChange),
    };
    Byte(byte: u8) { value: u8 = *byte; } => Ok::<_, ModelError>(*value);
    U16(number: u16) { value: u16 = *number; } => Ok::<_, ModelError>(*value);
    Gain(gain: u32) { bits: u32 = *gain; } => Ok::<_, ModelError>(*bits);
    Bits(float: u32) { bits: u32 = *float; } => Ok::<_, ModelError>(*bits);
    Flicks(flicks: i64) { value: i64 = *flicks; } => Ok::<_, ModelError>(*value);
    ParamIds(parameters: Vec<u32>) { ids: Vec<u32> = parameters.clone(); } => Ok::<_, ModelError>(ids.clone());
}

fn parse_id(text: &str) -> Result<Id, ModelError> {
    Id::parse(text).ok_or(ModelError::InvalidChange)
}
