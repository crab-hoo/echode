//! Realtime graph scheduler, transport, and playback.
//!
//! Playback planning does not allocate: callers pass the event and sample
//! buffers. The Phase 0 graph and device measurement require the `spike` feature.

#![cfg_attr(not(target_os = "macos"), allow(unused))]
// The audio thread runs this crate. Product code reaches buffers with `get`,
// iterators and slice patterns, so a bad index costs a block, not a panic
// (ADR 0284). The Phase 0 graph opts out to keep its measurement comparable.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

#[cfg(not(target_os = "macos"))]
compile_error!("Echode only builds for macOS");

mod audition;
mod builtin;
mod bypass;
mod clock;
mod declick;
mod device;
#[cfg(feature = "spike")]
mod graph;
mod mix;
mod outputs;
mod plan;
mod playback;
mod plugin_declick;
mod plugins;
mod realtime;
mod render;
mod resample;
mod resample_stream;
mod sequence;

#[cfg(feature = "spike")]
pub mod spike;
mod synth;
mod transport;
mod voice;
#[cfg(feature = "spike")]
mod wait;

pub use audition::{AuditionAudio, AuditionControl, AuditionSound, AuditionWriter, audition_pair};
pub use builtin::Device;
pub use bypass::copy_bypassed_audio;
pub use clock::{sample_to_tick, tick_to_sample};
pub use declick::OutputDeclick;
pub use device::{IoWorkgroup, OutputInventory, output_inventory};
pub use echode_realtime::Event;
pub use mix::{InputBus, MONO, STEREO, Speaker, TrackFader, mix_buses, mix_tracks};
mod meter;
pub use echode_sandbox::{
    GuiReport, ParamChange, ParamEvent, ParamEventKind, ParamInfo, ParamRelay, ParamText,
    ParamValue, PluginActivity, PluginCommand, PluginControl, RetainedWorkgroup, SessionError,
};
pub use meter::{AutoMute, TrackMeter};
pub use outputs::{DeviceFormat, OutputChoice, OutputRoute, RouteUpdate};
pub use plan::PlaybackIds;
pub use playback::{
    CachedTrack, ClipLoader, ClipWindow, ClockSource, InternalClock, LoadedClip, MASTER_GUARD_DBFS,
    MasterAutoMute, MediaInputs, PlaybackAudio, PlaybackCommand, PlaybackControl, PlaybackOptions,
    PlaybackStatus, playback,
};
pub use plugin_declick::PluginDeclick;
pub use plugins::{PluginRoute, PluginWorkgroup, block_duration};
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
pub use resample_stream::ResampleStream;
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
