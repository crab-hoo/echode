//! Tempo point metadata shared by the model, package serde, and replay.
macro_rules! tempo_fields {
    ($consumer:ident) => {
        $consumer! {
            Tempo => tempo: Tempo, Tempo;
            package {
                numerator: u32 = tempo.numerator();
                denominator: u32 = tempo.denominator();
            }
            restore Tempo::rational(numerator, denominator).ok_or("tempo is outside the allowed range");
        }
    };
}
pub(crate) use tempo_fields;
