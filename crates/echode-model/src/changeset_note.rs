//! Note fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Field, Id, ModelError, Note, ObjectKey, ObjectKind, Property, Tick, Value, field_byte,
    field_id, field_optional_id, field_tick, field_u16, update,
};

// The declaration order is the existing wire order. Identity and content ownership
// are separate: ownership is writable, but must agree with the content's note order.
macro_rules! note_fields {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $read:ident;)+) => {
        pub(super) struct NoteState {
            pub content: Id,
            $($field: $type,)+
        }

        pub(super) fn diff_note_fields(
            before_owner: Id,
            after_owner: Id,
            before: &Note,
            after: &Note,
            updates: &mut Vec<Change>,
        ) {
            if before_owner != after_owner {
                updates.push(update(
                    ObjectKey::Id(after.id), Property::Owner,
                    Value::Id(before_owner), Value::Id(after_owner),
                ));
            }
            $(
                if before.$field != after.$field {
                    updates.push(update(
                        ObjectKey::Id(after.id), Property::$property,
                        Value::$value(before.$field), Value::$value(after.$field),
                    ));
                }
            )+
        }

        pub(super) fn note_change(create: bool, content: Id, note: &Note) -> Change {
            let fields = vec![
                Field { property: Property::Owner, value: Value::Id(content) },
                $(Field { property: Property::$property, value: Value::$value(note.$field) },)+
            ];
            let id = ObjectKey::Id(note.id);
            if create {
                Change::Create { kind: ObjectKind::Note, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Note, id, fields }
            }
        }

        impl NoteState {
            pub fn from_note(content: Id, note: &Note) -> Self {
                Self { content, $($field: note.$field,)+ }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    content: field_id(fields, Property::Owner)?,
                    $($field: $read(fields, Property::$property)?,)+
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    (Property::Owner, Value::Id(content)) => {
                        self.content = *content;
                        Ok(())
                    }
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = *value;
                        Ok(())
                    },)+
                    // Preserve the legacy no-op accepted by Draft for all object ids.
                    (Property::Lane, Value::Text(_)) => Ok(()),
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id) -> Note {
                Note { id, $($field: self.$field,)+ }
            }
        }
    };
}

note_fields! {
    Start => start: Tick, Tick, field_tick;
    Length => length: Tick, Tick, field_tick;
    Pitch => pitch: u8, Byte, field_byte;
    Velocity => velocity: u16, U16, field_u16;
    Release => release: u16, U16, field_u16;
    Articulation => articulation: Option<Id>, Parent, field_optional_id;
}
