//! Device scalar fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Field, Id, ModelError, ObjectKey, ObjectKind, Property, Value, diff_parameter_fields,
    field_id, field_ids, field_optional_text, field_text, parameter_change, update,
};
use crate::device::{Device, DeviceKind, Parameter, valid_pins, valid_state_hash};
use crate::device_fields::device_fields;

// Identity, kind, ownership, and the separately resolved parameter order are not
// scalar fields. Creation and update orders differ in existing journals.
macro_rules! define_device_changeset {
    (
        fields { $(
            $(#[$meta:meta])*
            $property:ident => $field:ident: $type:ty,
            $encode:ident, $read:ident, $parse:ident; $ffi:ident $getter:ident;
        )+ }
        create [$($create:ident),+];
        diff [$($diff:ident),+];
    ) => {
        pub(super) struct DeviceState {
            pub track: Id,
            kind: String,
            $($field: $type,)+
            pub parameters: Vec<Id>,
        }

        #[derive(Clone, Copy)]
        enum DeviceField { $($property,)+ }

        impl DeviceField {
            fn create(self, device: &Device) -> Option<Field> {
                match self {
                    $(Self::$property => $encode(&device.$field).map(|value| Field {
                        property: Property::$property,
                        value,
                    }),)+
                }
            }

            fn diff(self, before: &Device, after: &Device, updates: &mut Vec<Change>) {
                match self {
                    $(Self::$property => {
                        if before.$field != after.$field {
                            // Absent optional text is omitted on creation and cleared
                            // with empty text on update, as in existing journals.
                            let value = |device: &Device| $encode(&device.$field)
                                .unwrap_or_else(|| Value::Text(String::new()));
                            updates.push(update(ObjectKey::Id(after.id), Property::$property,
                                value(before), value(after)));
                        }
                    },)+
                }
            }
        }

        pub(super) fn diff_device_fields(
            before: &Device,
            after: &Device,
            creates: &mut Vec<Change>,
            updates: &mut Vec<Change>,
            deletes: &mut Vec<Change>,
        ) {
            $(DeviceField::$diff.diff(before, after, updates);)+
            let before_order = parameter_order(before);
            let after_order = parameter_order(after);
            if before_order != after_order {
                updates.push(update(ObjectKey::Id(after.id), Property::ParamOrder,
                    Value::Ids(before_order), Value::Ids(after_order)));
            }
            for parameter in &after.parameters {
                if let Some(previous) = before.parameters.iter().find(|item| item.id == parameter.id) {
                    diff_parameter_fields(previous, parameter, updates);
                } else {
                    creates.push(parameter_change(true, after.id, parameter));
                }
            }
            for parameter in before.parameters.iter().rev() {
                if !after.parameters.iter().any(|item| item.id == parameter.id) {
                    deletes.push(parameter_change(false, before.id, parameter));
                }
            }
        }

        pub(super) fn device_change(create: bool, track: Id, device: &Device) -> Change {
            let mut fields = vec![
                Field { property: Property::Owner, value: Value::Id(track) },
                Field { property: Property::DeviceKind, value: Value::Text(device.kind.as_str().into()) },
            ];
            $(fields.extend(DeviceField::$create.create(device));)+
            fields.push(Field { property: Property::ParamOrder, value: Value::Ids(parameter_order(device)) });
            let id = ObjectKey::Id(device.id);
            if create {
                Change::Create { kind: ObjectKind::Device, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Device, id, fields }
            }
        }

        impl DeviceState {
            pub fn from_device(track: Id, device: &Device) -> Self {
                Self {
                    track,
                    kind: device.kind.as_str().to_string(),
                    $($field: device.$field.to_owned(),)+
                    parameters: parameter_order(device),
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    track: field_id(fields, Property::Owner)?,
                    kind: field_text(fields, Property::DeviceKind)?,
                    $($field: $read(fields, Property::$property)?,)+
                    parameters: field_ids(fields, Property::ParamOrder)?,
                })
            }

            pub fn is_scalar(property: Property) -> bool {
                matches!(property, $(Property::$property)|+)
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match property {
                    $(Property::$property => {
                        self.$field = $parse(value)?;
                        Ok(())
                    },)+
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id, parameters: Vec<Parameter>) -> Result<Device, ModelError> {
                let kind = DeviceKind::parse(&self.kind).ok_or(ModelError::InvalidChange)?;
                if kind == DeviceKind::Mixer
                    && (self.bypassed || self.offline || self.state.is_some() || !self.pinned.is_empty())
                {
                    return Err(ModelError::InvalidChange);
                }
                Ok(Device {
                    id,
                    kind,
                    $($field: self.$field.to_owned(),)+
                    parameters,
                })
            }
        }
    };
}

device_fields!(define_device_changeset);

fn parameter_order(device: &Device) -> Vec<Id> {
    device
        .parameters
        .iter()
        .map(|parameter| parameter.id)
        .collect()
}

fn flag_value(value: &bool) -> Option<Value> {
    Some(Value::Byte(u8::from(*value)))
}

fn pins_value(value: &[u32]) -> Option<Value> {
    Some(Value::ParamIds(value.to_vec()))
}

fn text_value(value: &Option<String>) -> Option<Value> {
    value.clone().map(Value::Text)
}

fn parse_flag(value: &Value) -> Result<bool, ModelError> {
    match value {
        Value::Byte(0) => Ok(false),
        Value::Byte(1) => Ok(true),
        _ => Err(ModelError::InvalidChange),
    }
}

fn parse_pins(value: &Value) -> Result<Vec<u32>, ModelError> {
    match value {
        Value::ParamIds(ids) if valid_pins(ids) => Ok(ids.clone()),
        _ => Err(ModelError::InvalidChange),
    }
}

fn parse_plugin(value: &Value) -> Result<Option<String>, ModelError> {
    match value {
        Value::Text(text) => Ok(Some(text.clone())),
        _ => Err(ModelError::InvalidChange),
    }
}

fn parse_state(value: &Value) -> Result<Option<String>, ModelError> {
    match value {
        Value::Text(text) if text.is_empty() => Ok(None),
        Value::Text(text) if valid_state_hash(text) => Ok(Some(text.clone())),
        _ => Err(ModelError::InvalidChange),
    }
}

// Missing flags and pins retain the defaults of journals written before them.
fn field_flag(fields: &[Field], property: Property) -> Result<bool, ModelError> {
    match fields.iter().find(|field| field.property == property) {
        None => Ok(false),
        Some(field) => parse_flag(&field.value),
    }
}

fn field_pinned(fields: &[Field], property: Property) -> Result<Vec<u32>, ModelError> {
    match fields.iter().find(|field| field.property == property) {
        None => Ok(Vec::new()),
        Some(field) => parse_pins(&field.value),
    }
}

fn field_state(fields: &[Field], property: Property) -> Result<Option<String>, ModelError> {
    let state = field_optional_text(fields, property)?;
    if state.as_deref().is_some_and(|hash| !valid_state_hash(hash)) {
        return Err(ModelError::InvalidChange);
    }
    Ok(state)
}
