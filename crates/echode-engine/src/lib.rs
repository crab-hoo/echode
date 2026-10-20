//! Realtime graph scheduler, transport, and the CoreAudio spike.
//!
//! Playback planning does not allocate: callers pass the event and sample
//! buffers. The 10-minute measurement lives in [`spike`].

#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod builtin;
mod bypass;
mod clock;
mod declick;
mod device;
mod graph;
mod handoff;
mod mix;
mod outputs;
mod plan;
mod playback;
mod plugins;
mod realtime;
mod render;
mod resample;
mod sequence;

pub mod spike;
mod synth;
mod transport;
mod voice;
mod wait;

pub use builtin::Device;
pub use bypass::copy_bypassed_audio;
pub use clock::{sample_to_tick, tick_to_sample};
pub use declick::OutputDeclick;
pub use device::{IoWorkgroup, OutputInventory, output_inventory};
pub use echode_realtime::Event;
pub use handoff::{Audio, Control, Cycle, pair};
pub use mix::{InputBus, MONO, STEREO, Speaker, TrackFader, mix_buses, mix_tracks};
mod meter;
pub use echode_sandbox::{ParamChange, ParamInfo, ParamValue, PluginControl, RetainedWorkgroup};
pub use meter::TrackMeter;
pub use outputs::{DeviceFormat, OutputChoice, OutputRoute, RouteUpdate};
pub use plan::PlaybackIds;
pub use playback::{
    CachedTrack, ClipLoader, ClockSource, InternalClock, LoadedClip, PlaybackAudio,
    PlaybackCommand, PlaybackControl, PlaybackStatus, playback_pair, playback_pair_at,
    playback_with_clock, playback_with_plugins, playback_with_plugins_at,
    playback_with_plugins_in_workgroup_at,
};
pub use plugins::{PluginRoute, PluginWorkgroup};
pub use realtime::ForbidAlloc;
#[cfg(debug_assertions)]
pub use realtime::GuardAlloc;
pub use render::{
    OfflineError, render_offline, render_offline_blocks, render_offline_blocks_with_clips,
    render_span_frames,
};
pub use resample::{
    CONVERTER_ID, CONVERTER_VERSION, ConverterMode, default_converter, resample_into, resample_with,
};
pub use sequence::{render_metronome, schedule};
pub use synth::{
    SAMPLER_INTERPOLATION, SamplerInterpolation, SamplerVoice, SynthVoice, TEST_SAMPLER_ID,
    TEST_SAMPLER_VERSION, TEST_SYNTH_ID, TEST_SYNTH_INSTRUMENT, TEST_SYNTH_VERSION, TestSampler,
    TestSynth, render_test_sampler, render_test_synth,
};
pub use transport::{PlaybackError, PlayedSpan, Transport};
pub use voice::{
    AudioClip, FadedClip, LoopHead, TrackSource, fade_loop_seam, loop_head_prefetch,
    render_audio_clips, render_faded_clips, render_track, seam_fade_frames,
};
