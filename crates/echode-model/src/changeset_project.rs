//! Project scalar fields shared by diff, draft conversion, and replay.

use super::{
    Arc, Change, Content, Meter, ModelError, ObjectKey, Project, Property, TempoMap, Track, Value,
    update,
};

// Declaration order is the existing update order and from_arcs argument order.
// Tempo points and children are resolved by Draft, not stored as scalar fields.
macro_rules! define_project_changes {
    ($($(#[$attribute:meta])* $property:ident => $field:ident: $type:ty, $value:ident;)+) => {
        pub(super) struct ProjectState {
            $($field: $type,)+
        }

        pub(super) fn diff_project_fields(before: &Project, after: &Project, updates: &mut Vec<Change>) {
            $(if before.$field() != after.$field() {
                updates.push(update(ObjectKey::Project, Property::$property,
                    Value::$value(before.$field()), Value::$value(after.$field())));
            })+
        }

        impl ProjectState {
            pub fn from_project(project: &Project) -> Self {
                Self { $($field: project.$field(),)+ }
            }

            pub fn set(&mut self, property: Property, value: &Value) -> Result<(), ModelError> {
                match (property, value) {
                    $((Property::$property, Value::$value(value)) => {
                        self.$field = *value;
                        Ok(())
                    },)+
                    // Legacy journals may contain this no-op; fractional track keys
                    // alone decide the arrange order. Do not store or emit this list.
                    (Property::TrackOrder, Value::Ids(_)) => Ok(()),
                    _ => Err(ModelError::InvalidChange),
                }
            }

            pub fn finish(self, tempo: TempoMap, contents: Vec<Arc<Content>>, tracks: Vec<Arc<Track>>) -> Project {
                Project::from_arcs($(self.$field,)+ tempo, contents, tracks)
            }
        }
    };
}

crate::project_fields::project_fields!(define_project_changes);
