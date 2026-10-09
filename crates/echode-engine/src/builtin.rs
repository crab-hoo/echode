//! In-process devices. Output hardware lives in [`crate::device`].
//!
//! `process` is the only method the audio thread calls. Preparing voices and
//! saving state run on the control thread and may allocate.

use echode_realtime::Event;

use crate::transport::PlaybackError;

/// A built-in instrument or effect, and later a hosted plugin.
///
/// Channel planes are whatever the caller passes. There is no fixed count.
pub trait Device {
    /// Adds this block into `planes`. Does not allocate or lock.
    fn process(
        &mut self,
        planes: &mut [&mut [f32]],
        events: &[Event],
        sample_rate: u32,
    ) -> Result<(), PlaybackError>;

    /// Sizes voice storage before playback. `voices` is how many may be busy at
    /// once, including release, not how many notes the score contains.
    fn prepare(&mut self, voices: usize, sample_rate: u32) -> Result<(), PlaybackError>;

    /// Writes the versioned configuration. Not the sounding voices or PCM.
    fn save(&self, out: &mut Vec<u8>);

    /// Restores configuration written by [`Device::save`].
    fn restore(&mut self, bytes: &[u8]) -> Result<(), PlaybackError>;

    fn latency_samples(&self) -> u32;

    fn tail_samples(&self, sample_rate: u32) -> u32;
}
