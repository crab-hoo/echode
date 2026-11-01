//! Content fields shared by diff, creation, draft conversion, and replay.

use super::{
    Change, Content, Field, Id, ModelError, Note, ObjectKey, ObjectKind, Property, Value,
    field_ids, note_order_change, update,
};
use crate::model::AudioRef;

// Creation keeps audio/takes before note order; updates keep note order first.
// Draft resolves the notes separately, checking ownership, reuse, and leftovers.
macro_rules! content_fields {
    ($($property:ident => $field:ident: $type:ty, $value:ident, $read:ident;)+) => {
        pub(super) struct ContentState {
            $($field: $type,)+
            pub note_order: Vec<Id>,
        }

        pub(super) fn diff_content_fields(before: &Content, after: &Content, updates: &mut Vec<Change>) {
            let before_notes = note_order(before);
            let after_notes = note_order(after);
            if before_notes != after_notes {
                updates.push(note_order_change(after.id, &before_notes, &after_notes));
            }
            $(
                if before.$field != after.$field {
                    updates.push(update(ObjectKey::Id(after.id), Property::$property,
                        Value::$value(before.$field.to_owned()), Value::$value(after.$field.to_owned())));
                }
            )+
        }

        pub(super) fn content_change(create: bool, content: &Content) -> Change {
            let fields = vec![
                $(Field { property: Property::$property, value: Value::$value(content.$field.to_owned()) },)+
                Field { property: Property::NoteOrder, value: Value::Ids(note_order(content)) },
            ];
            let id = ObjectKey::Id(content.id);
            if create {
                Change::Create { kind: ObjectKind::Content, id, fields }
            } else {
                Change::Delete { kind: ObjectKind::Content, id, fields }
            }
        }

        impl ContentState {
            pub fn from_content(content: &Content) -> Self {
                Self {
                    $($field: content.$field.to_owned(),)+
                    note_order: note_order(content),
                }
            }

            pub fn from_fields(fields: &[Field]) -> Result<Self, ModelError> {
                Ok(Self {
                    $($field: $read(fields, Property::$property)?,)+
                    note_order: field_ids(fields, Property::NoteOrder)?,
                })
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = value.to_owned();
                        Ok(())
                    },)+
                    // Journals written before audio files stored a null parent.
                    (Property::Audio, Value::Parent(None)) => self.set(Property::Audio, &Value::Audio(None)),
                    (Property::NoteOrder, Value::Ids(ids)) => {
                        self.note_order = ids.clone();
                        Ok(())
                    }
                    (Property::NoteOrder, Value::OrderStep { index, id, present }) => {
                        set_order_step(&mut self.note_order, *index, *id, *present)
                    }
                    // Preserve the legacy no-op accepted by Draft for all object ids.
                    (Property::Lane, Value::Text(_)) => Ok(()),
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(&self, id: Id, notes: Vec<Note>) -> Content {
                Content { id, $($field: self.$field.to_owned(),)+ notes, expressions: Vec::new() }
            }
        }
    };
}

content_fields! {
    Audio => audio: Option<AudioRef>, Audio, audio_field;
    Takes => takes: Vec<Id>, Ids, field_ids;
}

fn note_order(content: &Content) -> Vec<Id> {
    content.notes.iter().map(|note| note.id).collect()
}

fn audio_field(fields: &[Field], property: Property) -> Result<Option<AudioRef>, ModelError> {
    let field = fields
        .iter()
        .find(|field| field.property == property)
        .ok_or(ModelError::InvalidChange)?;
    match &field.value {
        Value::Audio(audio) => Ok(audio.clone()),
        Value::Parent(None) => Ok(None),
        _ => Err(ModelError::InvalidChange),
    }
}

fn set_order_step(
    order: &mut Vec<Id>,
    index: u32,
    note: Id,
    present: bool,
) -> Result<(), ModelError> {
    let index = usize::try_from(index).map_err(|_| ModelError::InvalidChange)?;
    if present {
        if index > order.len() {
            return Err(ModelError::InvalidChange);
        }
        order.insert(index, note);
    } else if order.get(index) == Some(&note) {
        order.remove(index);
    } else {
        return Err(ModelError::InvalidChange);
    }
    Ok(())
}
