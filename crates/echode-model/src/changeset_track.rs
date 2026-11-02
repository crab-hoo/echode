//! Track scalar fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Device, Field, Id, ModelError, ObjectKey, ObjectKind, Placement, Property, Track,
    Value, field_ids, field_optional_id, field_text, update,
};
use crate::track_fields::track_fields;

// Declaration order is the existing wire order. Child orders are resolved by Draft;
// hierarchy and fractional keys are validated by arrange_tracks after replay.
macro_rules! define_track_changeset {
    ($($(#[$attribute:meta])* $property:ident => $field:ident: $type:ty, $value:ident, $read:ident;)+) => {
        pub(super) struct TrackState {
            $($field: $type,)+
            pub clip_order: Vec<Id>,
            pub device_order: Vec<Id>,
        }

        pub(super) fn diff_track_fields(before: &Track, after: &Track, updates: &mut Vec<Change>) {
            $(
                if before.$field != after.$field {
                    updates.push(update(ObjectKey::Id(after.id), Property::$property,
                        Value::$value(before.$field.to_owned()), Value::$value(after.$field.to_owned())));
                }
            )+
            for (property, before_ids, after_ids) in [
                (Property::ClipOrder, clip_order(before), clip_order(after)),
                (Property::DeviceOrder, device_order(before), device_order(after)),
            ] {
                if before_ids != after_ids {
                    updates.push(update(ObjectKey::Id(after.id), property,
                        Value::Ids(before_ids), Value::Ids(after_ids)));
                }
            }
        }

        pub(super) fn track_change(create: bool, track: &Track) -> Change {
            let fields = vec![
                $(Field { property: Property::$property, value: Value::$value(track.$field.to_owned()) },)+
                Field { property: Property::ClipOrder, value: Value::Ids(clip_order(track)) },
                Field { property: Property::DeviceOrder, value: Value::Ids(device_order(track)) },
            ];
            let id = ObjectKey::Id(track.id);
            if create {
                Change::Create { kind: ObjectKind::Track, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Track, id, fields }
            }
        }

        impl TrackState {
            pub fn from_track(track: &Track) -> Self {
                Self {
                    $($field: track.$field.to_owned(),)+
                    clip_order: clip_order(track),
                    device_order: device_order(track),
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    $($field: $read(fields, Property::$property)?,)+
                    clip_order: field_ids(fields, Property::ClipOrder)?,
                    device_order: field_ids(fields, Property::DeviceOrder)?,
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = value.to_owned();
                        Ok(())
                    },)+
                    (Property::ClipOrder, Value::Ids(ids)) => {
                        self.clip_order = ids.clone();
                        Ok(())
                    }
                    (Property::DeviceOrder, Value::Ids(ids)) => {
                        self.device_order = ids.clone();
                        Ok(())
                    }
                    // Preserve the legacy no-op accepted by Draft for all object ids.
                    (Property::Lane, Value::Text(_)) => Ok(()),
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id, clips: Vec<Placement>, devices: Vec<Device>) -> Track {
                Track { id, $($field: self.$field.to_owned(),)+ clips, devices }
            }
        }
    };
}

track_fields!(define_track_changeset);

fn clip_order(track: &Track) -> Vec<Id> {
    track.clips.iter().map(|clip| clip.id).collect()
}

fn device_order(track: &Track) -> Vec<Id> {
    track.devices.iter().map(|device| device.id).collect()
}
