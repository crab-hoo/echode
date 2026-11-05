//! Host-independent instrument DSP shared by the bundled plugin and engine fixtures.

mod allocation;
mod sine_pad;

pub use allocation::ForbidAlloc;
#[cfg(debug_assertions)]
pub use allocation::GuardAlloc;
pub use sine_pad::{INSTRUMENT, SYNTH_ID, SYNTH_VERSION, SinePad, SynthVoice, render_sine};

/// Invalid DSP configuration or caller-prepared storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspError {
    InvalidSampleRate,
    BufferTooSmall,
    Time,
    InvalidState,
}

impl std::fmt::Display for DspError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSampleRate => "sample rate must be above zero and match prepared voices",
            Self::BufferTooSmall => "the caller buffer cannot hold this block",
            Self::Time => "event positions must be ordered and representable",
            Self::InvalidState => "instrument state is not valid",
        })
    }
}

impl std::error::Error for DspError {}
