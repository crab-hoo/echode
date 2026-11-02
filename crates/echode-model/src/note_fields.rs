//! Note scalars shared by the model, package serde, and changesets.

// Declaration order is both the package key order and the journal field order.
// Identity and content ownership are kept outside this scalar table.
macro_rules! note_fields {
    ($consumer:ident) => {
        $consumer! {
            Start => start: Tick, Tick, field_tick;
            Length => length: Tick, Tick, field_tick;
            Pitch => pitch: u8, Byte, field_byte;
            Velocity => velocity: u16, U16, field_u16;
            Release => release: u16, U16, field_u16;
            Articulation => articulation: Option<Id>, Parent, field_optional_id;
        }
    };
}

pub(crate) use note_fields;
