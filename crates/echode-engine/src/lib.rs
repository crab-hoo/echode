//! Realtime graph scheduler, transport, and the CoreAudio spike.
//!
//! Playback planning does not allocate: callers pass the event and sample
//! buffers. The spike graph in [`graph`] is a separate measurement workload.

#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod clock;
mod device;
mod graph;
mod mix;
mod outputs;
mod realtime;
mod render;
mod resample;
mod sequence;
mod synth;
mod transport;
mod voice;
mod wait;

pub use clock::{sample_to_tick, tick_to_sample};
pub use device::{OutputInventory, Report, install_stop_handler, output_inventory, run};
pub use graph::{AUDIO_WORKERS, BLOCK_FRAMES, Engine, NODE_COUNT};
pub use mix::{TrackFader, mix_tracks};
pub use outputs::{DeviceFormat, OutputChoice, OutputRoute, RouteUpdate};
pub use render::{render_offline, render_span_frames};
pub use resample::{
    CONVERTER_ID, CONVERTER_VERSION, ConverterMode, default_converter, resample_into, resample_with,
};
pub use sequence::{MidiEvent, MidiKind, render_metronome, schedule};
pub use synth::{
    SamplerInterpolation, SamplerVoice, SynthVoice, TEST_SAMPLER_ID, TEST_SAMPLER_VERSION,
    TEST_SYNTH_ID, TEST_SYNTH_INSTRUMENT, TEST_SYNTH_VERSION, render_test_sampler,
    render_test_synth,
};
pub use transport::{PlaybackError, PlayedSpan, Transport};
pub use voice::{
    AudioClip, LoopHead, TrackSource, fade_loop_seam, loop_head_prefetch, render_audio_clips,
    render_track, seam_fade_frames,
};
