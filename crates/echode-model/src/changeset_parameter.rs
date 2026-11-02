//! Parameter field mapping shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Field, Id, ModelError, ObjectKey, ObjectKind, Property, Value, field_bits, field_id,
    field_text, update,
};
use crate::device::{ParamKey, Parameter};
use crate::parameter_fields::parameter_fields;

// Keep the wire order and float-bit representation explicit. Identity, ownership,
// and the immutable key are not writable parameter fields.
macro_rules! define_parameter_changes {
    ($($property:ident => $field:ident),+ $(,)?) => {
        pub(super) struct ParamState {
            pub device: Id,
            key: String,
            $($field: u32,)+
        }

        pub(super) fn diff_parameter_fields(
            before: &Parameter,
            after: &Parameter,
            updates: &mut Vec<Change>,
        ) {
            $(
                if before.$field.to_bits() != after.$field.to_bits() {
                    updates.push(update(
                        ObjectKey::Id(after.id),
                        Property::$property,
                        Value::Bits(before.$field.to_bits()),
                        Value::Bits(after.$field.to_bits()),
                    ));
                }
            )+
        }

        pub(super) fn parameter_change(create: bool, device: Id, parameter: &Parameter) -> Change {
            let fields = vec![
                Field {
                    property: Property::Owner,
                    value: Value::Id(device),
                },
                Field {
                    property: Property::Key,
                    value: Value::Text(parameter.key.as_str().to_string()),
                },
                $(
                    Field {
                        property: Property::$property,
                        value: Value::Bits(parameter.$field.to_bits()),
                    },
                )+
            ];
            let id = ObjectKey::Id(parameter.id);
            if create {
                Change::Create { kind: ObjectKind::Parameter, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Parameter, id, fields }
            }
        }

        impl ParamState {
            pub fn from_parameter(device: Id, parameter: &Parameter) -> Self {
                Self {
                    device,
                    key: parameter.key.as_str().to_string(),
                    $($field: parameter.$field.to_bits(),)+
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    device: field_id(fields, Property::Owner)?,
                    key: field_text(fields, Property::Key)?,
                    $($field: field_bits(fields, Property::$property)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    $((Property::$property, Value::Bits(bits)) => {
                        self.$field = *bits;
                        Ok(())
                    },)+
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id) -> Result<Parameter, ModelError> {
                Ok(Parameter {
                    id,
                    key: ParamKey::parse(&self.key).ok_or(ModelError::InvalidChange)?,
                    $($field: f32::from_bits(self.$field),)+
                })
            }
        }
    };
}

parameter_fields!(define_parameter_changes);
