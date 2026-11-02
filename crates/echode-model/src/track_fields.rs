//! Track scalars shared by the model, package metadata, and changesets.

// Identity and child resolution stay outside this table. Declaration order is
// both the package scalar key order and the journal scalar field order.
macro_rules! track_fields {
    ($consumer:ident) => {
        $consumer! {
            Name => name: String, Text, field_text;
            Parent => parent: Option<Id>, Parent, field_optional_id;
            /// Sibling order under `parent`. Compared as a fractional index string.
            Order => order: String, Text, field_text;
        }
    };
}

pub(crate) use track_fields;
