//! Placement fields shared by diff, creation, draft conversion, and replay.

use std::convert::identity;

use super::{
    Change, ClipKind, Field, Id, ModelError, ObjectKey, ObjectKind, Placement, Property, Tick,
    Value, field_id, field_tick, update,
};
use crate::model::Lane;

// Declaration order is creation order; updates keep the existing different order.
// Track ownership and content existence are checked by Draft, not by this table.
macro_rules! placement_fields {
    (
        fields { $(
            $property:ident => $visibility:vis $field:ident: $type:ty, $value:ident,
            $read:ident, $encode:path, $decode:path;
        )+ }
        diff [$($diff:ident),+];
    ) => {
        pub(super) struct ClipState {
            pub track: Id,
            $($visibility $field: $type,)+
        }

        enum PlacementField { $($property,)+ }

        impl PlacementField {
            fn diff(self, before: &ClipState, after: &ClipState, id: Id, updates: &mut Vec<Change>) {
                match self {
                    $(Self::$property => {
                        if before.$field != after.$field {
                            updates.push(update(ObjectKey::Id(id), Property::$property,
                                Value::$value(before.$field), Value::$value(after.$field)));
                        }
                    },)+
                }
            }
        }

        pub(super) fn diff_clip_fields(
            before_owner: Id,
            after_owner: Id,
            before: &Placement,
            after: &Placement,
            updates: &mut Vec<Change>,
        ) {
            let id = after.id;
            let before = ClipState::from_clip(before_owner, before);
            let after = ClipState::from_clip(after_owner, after);
            if before.track != after.track {
                updates.push(update(ObjectKey::Id(id), Property::Owner,
                    Value::Id(before.track), Value::Id(after.track)));
            }
            $(PlacementField::$diff.diff(&before, &after, id, updates);)+
        }

        pub(super) fn clip_change(create: bool, track: Id, clip: &Placement) -> Change {
            let state = ClipState::from_clip(track, clip);
            let fields = vec![
                Field { property: Property::Owner, value: Value::Id(track) },
                $(Field { property: Property::$property, value: Value::$value(state.$field) },)+
                Field { property: Property::Lane, value: Value::Text("timeline".into()) },
            ];
            let id = ObjectKey::Id(clip.id);
            if create {
                Change::Create { kind: ObjectKind::Clip, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Clip, id, fields }
            }
        }

        impl ClipState {
            pub fn from_clip(track: Id, clip: &Placement) -> Self {
                Self { track, $($field: $encode(clip.$field),)+ }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    track: field_id(fields, Property::Owner)?,
                    $($field: $read(fields, Property::$property)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    (Property::Owner, Value::Id(track)) => {
                        self.track = *track;
                        Ok(())
                    }
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = *value;
                        Ok(())
                    },)+
                    // Preserve the legacy no-op; only the timeline lane exists.
                    (Property::Lane, Value::Text(_)) => Ok(()),
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id) -> Placement {
                Placement { id, lane: Lane::Timeline, $($field: $decode(self.$field),)+ }
            }
        }
    };
}

placement_fields! {
    fields {
        Content => pub content: Id, Id, field_id, identity, identity;
        Start => start: Tick, Tick, field_tick, identity, identity;
        Length => length: Tick, Tick, field_tick, identity, identity;
        Offset => offset: Tick, Tick, field_tick, identity, identity;
        Kind => kind: ClipKind, Kind, field_kind, identity, identity;
        Gain => gain: u32, Gain, field_gain, f32::to_bits, f32::from_bits;
        FadeIn => fade_in: i64, Flicks, field_flicks, identity, identity;
        FadeOut => fade_out: i64, Flicks, field_flicks, identity, identity;
    }
    diff [Start, Length, Kind, Offset, Content, Gain, FadeIn, FadeOut];
}

fn field_kind(fields: &[Field], property: Property) -> Result<ClipKind, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Kind(kind),
            } if *found == property => Some(*kind),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_gain(fields: &[Field], property: Property) -> Result<u32, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Gain(bits),
            } if *found == property => Some(*bits),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}

fn field_flicks(fields: &[Field], property: Property) -> Result<i64, ModelError> {
    fields
        .iter()
        .find_map(|field| match field {
            Field {
                property: found,
                value: Value::Flicks(flicks),
            } if *found == property => Some(*flicks),
            _ => None,
        })
        .ok_or(ModelError::InvalidChange)
}
