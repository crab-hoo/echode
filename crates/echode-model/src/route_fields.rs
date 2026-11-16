//! Single tables of send and sidechain fields shared by model structs, package
//! serialization, and changeset conversion.

macro_rules! send_fields {
    ($call:ident) => {
        $call! {
            Target => target: crate::Id, Id, field_id, parse_id; skip none;
            Tap => tap: crate::routing::RouteTap, Text, field_tap, parse_tap; skip none;
            Gain => gain: f32, Bits, field_f32, parse_f32; skip none;
        }
    };
}

macro_rules! sidechain_fields {
    ($call:ident) => {
        $call! {
            Source => source: crate::Id, Id, field_id, parse_id; skip none;
            Tap => tap: crate::routing::RouteTap, Text, field_tap, parse_tap; skip none;
            Gain => gain: f32, Bits, field_f32, parse_f32; skip none;
        }
    };
}

pub(crate) use {send_fields, sidechain_fields};
