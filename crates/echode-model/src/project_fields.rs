//! Project metadata shared by model declaration, package assembly, and replay.
macro_rules! project_fields {
    ($consumer:ident) => {
        $consumer! {
            #[serde(with = "crate::package_fields::meter")]
            Meter => meter: Meter, Meter;
        }
    };
}
pub(crate) use project_fields;
