//! Tempo point fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Field, ModelError, ObjectKey, ObjectKind, Project, Property, Tempo, TempoPoint, Tick,
    Value, update,
};
use std::collections::HashMap;

// Tick is the point's identity, not a writable field. Draft validates the map.
macro_rules! tempo_fields {
    ($($property:ident => $field:ident: $type:ty, $value:ident;)+) => {
        pub(super) struct TempoState {
            $($field: $type,)+
        }

        fn diff_tempo_fields(before: &TempoPoint, after: &TempoPoint, updates: &mut Vec<Change>) {
            $(if before.$field != after.$field {
                updates.push(update(ObjectKey::Tempo(after.at), Property::$property,
                    Value::$value(before.$field), Value::$value(after.$field)));
            })+
        }

        fn tempo_change(create: bool, point: &TempoPoint) -> Change {
            let fields = vec![$(Field {
                property: Property::$property, value: Value::$value(point.$field),
            },)+];
            let id = ObjectKey::Tempo(point.at);
            if create {
                Change::Create { kind: ObjectKind::TempoPoint, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::TempoPoint, id, fields }
            }
        }

        impl TempoState {
            pub fn from_point(point: &TempoPoint) -> Self {
                Self { $($field: point.$field,)+ }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    $($field: fields.iter().find_map(|field| match field {
                        Field { property: Property::$property, value: Value::$value(value) } => Some(*value),
                        _ => None,
                    }).ok_or(ModelError::InvalidChange)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = *value;
                        Ok(())
                    },)+
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, at: Tick) -> TempoPoint {
                TempoPoint { at, $($field: self.$field,)+ }
            }
        }
    };
}

tempo_fields! {
    Tempo => tempo: Tempo, Tempo;
}

pub(super) fn diff_tempo(
    before: &Project,
    after: &Project,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    let previous: HashMap<i64, &TempoPoint> = before
        .tempo()
        .points()
        .iter()
        .map(|point| (point.at.get(), point))
        .collect();
    let next: HashMap<i64, &TempoPoint> = after
        .tempo()
        .points()
        .iter()
        .map(|point| (point.at.get(), point))
        .collect();
    let mut created: Vec<i64> = next
        .keys()
        .filter(|tick| !previous.contains_key(tick))
        .copied()
        .collect();
    created.sort_unstable();
    for tick in created {
        creates.push(tempo_change(true, next[&tick]));
    }
    let mut revised: Vec<i64> = next
        .iter()
        .filter(|(tick, point)| previous.get(tick).is_some_and(|before| before != *point))
        .map(|(tick, _)| *tick)
        .collect();
    revised.sort_unstable();
    for tick in revised {
        diff_tempo_fields(previous[&tick], next[&tick], updates);
    }
    let mut removed: Vec<i64> = previous
        .keys()
        .filter(|tick| !next.contains_key(tick))
        .copied()
        .collect();
    removed.sort_unstable();
    for tick in removed {
        deletes.push(tempo_change(false, previous[&tick]));
    }
}
