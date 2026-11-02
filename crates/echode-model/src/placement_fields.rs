//! Placement scalars, replay codecs, and the existing snapshot projection.
// The public C layout remains in echode-ffi; its scalar assignments come from
// this table. `skip` explicitly marks fields not exposed by that ABI.
#[doc(hidden)]
#[macro_export]
macro_rules! placement_fields {
    ($consumer:ident) => {
        $consumer! {
            fields {
                Content => pub content: $crate::Id => $crate::Id, Id, field_id, identity, identity; skip;
                Start => start: $crate::Tick => $crate::Tick, Tick, field_tick, identity, identity; ticks;
                Length => length: $crate::Tick => $crate::Tick, Tick, field_tick, identity, identity; ticks;
                Offset => offset: $crate::Tick => $crate::Tick, Tick, field_tick, identity, identity; skip;
                Kind => kind: $crate::ClipKind => $crate::ClipKind, Kind, field_kind, identity, identity; skip;
                Gain => gain: f32 => u32, Gain, field_gain, f32::to_bits, f32::from_bits; copy;
                FadeIn => fade_in: i64 => i64, Flicks, field_flicks, identity, identity; copy;
                FadeOut => fade_out: i64 => i64, Flicks, field_flicks, identity, identity; copy;
            }
            diff [Start, Length, Kind, Offset, Content, Gain, FadeIn, FadeOut];
        }
    };
}
