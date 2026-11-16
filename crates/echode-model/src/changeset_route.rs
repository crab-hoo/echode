//! Send and sidechain field mapping shared by diff, creation, draft conversion,
//! and changeset replay.

use super::{
    Change, Field, Id, ModelError, ObjectKey, ObjectKind, Property, Value, field_bits, field_id,
    field_text, update,
};
use crate::route_fields::{send_fields, sidechain_fields};
use crate::routing::{RouteTap, SendRoute, SidechainRoute};

pub(super) struct SendState {
    pub track: Id,
    pub target: Id,
    tap: String,
    gain: u32,
}

pub(super) struct SidechainState {
    pub device: Id,
    pub source: Id,
    tap: String,
    gain: u32,
}

fn field_f32(value: f32) -> Value {
    Value::Bits(value.to_bits())
}

fn field_tap(tap: RouteTap) -> Value {
    Value::Text(tap.as_str().to_string())
}

fn emit_id(id: Id) -> Value {
    Value::Id(id)
}

fn parse_f32(fields: &[Field], property: Property) -> Result<u32, ModelError> {
    field_bits(fields, property)
}

fn parse_tap(fields: &[Field], property: Property) -> Result<String, ModelError> {
    field_text(fields, property)
}

fn parse_id(fields: &[Field], property: Property) -> Result<Id, ModelError> {
    field_id(fields, property)
}

macro_rules! emit_field {
    (field_id, $value:expr) => {
        emit_id($value)
    };
    ($emit:ident, $value:expr) => {
        $emit($value)
    };
}

macro_rules! diff_field {
    ($id:expr, $updates:expr, Bits, $property:ident, $before:expr, $after:expr) => {
        if $before.to_bits() != $after.to_bits() {
            $updates.push(update(
                ObjectKey::Id($id),
                Property::$property,
                Value::Bits($before.to_bits()),
                Value::Bits($after.to_bits()),
            ));
        }
    };
    ($id:expr, $updates:expr, Text, $property:ident, $before:expr, $after:expr) => {
        if $before != $after {
            $updates.push(update(
                ObjectKey::Id($id),
                Property::$property,
                field_tap($before),
                field_tap($after),
            ));
        }
    };
    ($id:expr, $updates:expr, Id, $property:ident, $before:expr, $after:expr) => {
        if $before != $after {
            $updates.push(update(
                ObjectKey::Id($id),
                Property::$property,
                Value::Id($before),
                Value::Id($after),
            ));
        }
    };
}

macro_rules! define_send_changes {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $emit:ident, $parse:ident; skip $skip:ident;)+) => {
        pub(super) fn diff_send_fields(
            before: &SendRoute,
            after: &SendRoute,
            updates: &mut Vec<Change>,
        ) {
            $(diff_field!(after.id, updates, $value, $property, before.$field, after.$field);)+
        }

        pub(super) fn send_change(create: bool, track: Id, send: &SendRoute) -> Change {
            let fields = vec![
                Field {
                    property: Property::Owner,
                    value: Value::Id(track),
                },
                $(
                    Field {
                        property: Property::$property,
                        value: emit_field!($emit, send.$field),
                    },
                )+
            ];
            let id = ObjectKey::Id(send.id);
            if create {
                Change::Create { kind: ObjectKind::Send, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Send, id, fields }
            }
        }

        impl SendState {
            pub fn from_send(track: Id, send: &SendRoute) -> Self {
                Self {
                    track,
                    target: send.target,
                    tap: send.tap.as_str().to_string(),
                    gain: send.gain.to_bits(),
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    track: field_id(fields, Property::Owner)?,
                    $($field: $parse(fields, Property::$property)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    (Property::Target, Value::Id(target)) => {
                        self.target = *target;
                        Ok(())
                    }
                    (Property::Tap, Value::Text(tap)) => {
                        if RouteTap::parse(tap).is_none() {
                            return Err(ModelError::InvalidChange);
                        }
                        self.tap = tap.clone();
                        Ok(())
                    }
                    (Property::Gain, Value::Bits(bits)) => {
                        self.gain = *bits;
                        Ok(())
                    }
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id) -> Result<SendRoute, ModelError> {
                Ok(SendRoute {
                    id,
                    target: self.target,
                    tap: RouteTap::parse(&self.tap).ok_or(ModelError::InvalidChange)?,
                    gain: f32::from_bits(self.gain),
                })
            }
        }
    };
}

send_fields!(define_send_changes);

macro_rules! define_sidechain_changes {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $emit:ident, $parse:ident; skip $skip:ident;)+) => {
        pub(super) fn diff_sidechain_fields(
            before: &SidechainRoute,
            after: &SidechainRoute,
            updates: &mut Vec<Change>,
        ) {
            $(diff_field!(after.id, updates, $value, $property, before.$field, after.$field);)+
        }

        pub(super) fn sidechain_change(
            create: bool,
            device: Id,
            sidechain: &SidechainRoute,
        ) -> Change {
            let fields = vec![
                Field {
                    property: Property::Owner,
                    value: Value::Id(device),
                },
                $(
                    Field {
                        property: Property::$property,
                        value: emit_field!($emit, sidechain.$field),
                    },
                )+
            ];
            let id = ObjectKey::Id(sidechain.id);
            if create {
                Change::Create { kind: ObjectKind::Sidechain, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Sidechain, id, fields }
            }
        }

        impl SidechainState {
            pub fn from_sidechain(device: Id, sidechain: &SidechainRoute) -> Self {
                Self {
                    device,
                    source: sidechain.source,
                    tap: sidechain.tap.as_str().to_string(),
                    gain: sidechain.gain.to_bits(),
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    device: field_id(fields, Property::Owner)?,
                    $($field: $parse(fields, Property::$property)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    (Property::Source, Value::Id(source)) => {
                        self.source = *source;
                        Ok(())
                    }
                    (Property::Tap, Value::Text(tap)) => {
                        if RouteTap::parse(tap).is_none() {
                            return Err(ModelError::InvalidChange);
                        }
                        self.tap = tap.clone();
                        Ok(())
                    }
                    (Property::Gain, Value::Bits(bits)) => {
                        self.gain = *bits;
                        Ok(())
                    }
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id) -> Result<SidechainRoute, ModelError> {
                Ok(SidechainRoute {
                    id,
                    source: self.source,
                    tap: RouteTap::parse(&self.tap).ok_or(ModelError::InvalidChange)?,
                    gain: f32::from_bits(self.gain),
                })
            }
        }
    };
}

sidechain_fields!(define_sidechain_changes);
