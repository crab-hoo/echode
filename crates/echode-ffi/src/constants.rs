//! Authoritative model units and value bounds copied through the C ABI.

/// Immutable model constants. This is a value copy, with no document or pointer ownership.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ModelConstants {
    pub ticks_per_quarter: i64,
    pub flicks_per_second: i64,
    pub gain_max: f32,
    pub pan_min: f32,
    pub pan_max: f32,
    pub clip_gain_max: f32,
    pub fade_rate_limit: u32,
    pub midi7_max: u32,
}

/// Returns the linked model's units and bounds without opening a document or device.
#[unsafe(no_mangle)]
pub extern "C" fn echode_model_constants() -> ModelConstants {
    ModelConstants {
        ticks_per_quarter: echode_model::TICKS_PER_QUARTER,
        flicks_per_second: echode_model::FLICKS_PER_SECOND,
        gain_max: echode_model::GAIN_MAX,
        pan_min: echode_model::PAN_MIN,
        pan_max: echode_model::PAN_MAX,
        clip_gain_max: echode_model::CLIP_GAIN_MAX,
        fade_rate_limit: echode_model::FADE_RATE_LIMIT,
        midi7_max: u32::from(echode_model::MIDI7_MAX),
    }
}
