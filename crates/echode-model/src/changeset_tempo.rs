//! Tempo point fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Field, ModelError, ObjectKey, ObjectKind, Project, Property, Tempo, TempoPoint, Tick,
    Value, update,
};
use std::cmp::Ordering;

// Tick is the point's identity, not a writable field. Draft validates the map.
macro_rules! define_tempo_changes {
    ($($property:ident => $field:ident: $type:ty, $value:ident;
        package { $($wire:ident: $wire_type:ty = $encode:expr;)+ }
        restore $decode:expr;)+) => {
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

crate::tempo_fields::tempo_fields!(define_tempo_changes);

pub(super) fn diff_tempo(
    before: &Project,
    after: &Project,
    creates: &mut Vec<Change>,
    updates: &mut Vec<Change>,
    deletes: &mut Vec<Change>,
) {
    if before.tempo().shares_points(after.tempo()) {
        return;
    }
    // Both maps are already sorted by signed tick. Keep the existing separate
    // create/update/delete buffers, without rebuilding or sorting identity maps.
    let mut previous = tempo_points(before).peekable();
    let mut next = tempo_points(after).peekable();
    loop {
        match (previous.peek(), next.peek()) {
            (Some(left), Some(right)) => match left.at.cmp(&right.at) {
                Ordering::Less => {
                    deletes.push(tempo_change(false, left));
                    previous.next();
                }
                Ordering::Greater => {
                    creates.push(tempo_change(true, right));
                    next.next();
                }
                Ordering::Equal => {
                    diff_tempo_fields(left, right, updates);
                    previous.next();
                    next.next();
                }
            },
            (Some(_), None) => {
                deletes.extend(previous.map(|point| tempo_change(false, point)));
                break;
            }
            (None, Some(_)) => {
                creates.extend(next.map(|point| tempo_change(true, point)));
                break;
            }
            (None, None) => break,
        }
    }
}

fn tempo_points(project: &Project) -> impl Iterator<Item = &TempoPoint> {
    project.tempo().points().iter().inspect(|_| {
        #[cfg(test)]
        crate::model_scale_tests::diff_tempo_visit();
    })
}
