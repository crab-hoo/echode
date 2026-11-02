//! Content scalars shared by the model, package metadata, and changesets.
macro_rules! content_fields {
    ($consumer:ident) => {
        $consumer! {
            Audio => audio: Option<AudioRef>, Audio, audio_field;
            Takes => takes: Vec<Id>, Ids, field_ids;
        }
    };
}
pub(crate) use content_fields;
