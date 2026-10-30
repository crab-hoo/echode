use echode_ffi::echode_model_constants;
use echode_model::{
    CLIP_GAIN_MAX, FADE_RATE_LIMIT, FLICKS_PER_SECOND, GAIN_MAX, MIDI7_MAX, PAN_MAX, PAN_MIN,
    TICKS_PER_QUARTER,
};

#[test]
fn contract_model_constants_match_the_authoritative_units_and_bounds() {
    let constants = echode_model_constants();
    assert_eq!(constants.ticks_per_quarter, TICKS_PER_QUARTER);
    assert_eq!(constants.flicks_per_second, FLICKS_PER_SECOND);
    assert_eq!(constants.gain_max, GAIN_MAX);
    assert_eq!(constants.pan_min, PAN_MIN);
    assert_eq!(constants.pan_max, PAN_MAX);
    assert_eq!(constants.clip_gain_max, CLIP_GAIN_MAX);
    assert_eq!(constants.fade_rate_limit, FADE_RATE_LIMIT);
    assert_eq!(constants.midi7_max, u32::from(MIDI7_MAX));
}
