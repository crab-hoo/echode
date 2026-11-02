//! Parameter scalars shared by the model, package serde, and changesets.

// Declaration order is both the package key order and the changeset wire order.
// Identity and the immutable key stay outside the writable scalar list.
macro_rules! parameter_fields {
    ($consumer:ident) => {
        $consumer! {
            Minimum => min,
            Maximum => max,
            Default => default,
            Value => value,
        }
    };
}

pub(crate) use parameter_fields;
