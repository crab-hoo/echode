//! Incremental control-side SRC using the same rubato FFT settings as clips.
//! One converter spans the entire file; only its startup delay is trimmed.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::PlaybackError;

pub struct ResampleStream {
    converter: Option<Fft<f32>>,
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    input: u64,
    output: u64,
    trim: usize,
    done: bool,
}

impl ResampleStream {
    pub fn new(input_rate: u32, output_rate: u32, channels: u16) -> Result<Self, PlaybackError> {
        if input_rate == 0 || output_rate == 0 {
            return Err(PlaybackError::InvalidSampleRate);
        }
        if !matches!(channels, 1 | 2) {
            return Err(PlaybackError::Media);
        }
        let converter = if input_rate == output_rate {
            None
        } else {
            Some(
                Fft::new(
                    input_rate as usize,
                    output_rate as usize,
                    1024,
                    usize::from(channels),
                    FixedSync::Both,
                )
                .map_err(|_| PlaybackError::InvalidSampleRate)?,
            )
        };
        let trim = converter.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            converter,
            channels: usize::from(channels),
            input_rate,
            output_rate,
            input: 0,
            output: 0,
            trim,
            done: false,
        })
    }

    /// The next source read size. This is a processing chunk, not a file limit.
    pub fn input_frames(&self) -> usize {
        self.converter
            .as_ref()
            .map_or(1024, Resampler::input_frames_next)
    }

    /// Converts one source read; `eof` also drains the converter's delay.
    /// A partial read is only valid at EOF. All allocation stays on control.
    pub fn process(&mut self, samples: &[f32], eof: bool) -> Result<Vec<[f32; 2]>, PlaybackError> {
        let frames = samples.len() / self.channels;
        if self.done
            || !samples.len().is_multiple_of(self.channels)
            || frames > self.input_frames()
            || (!eof && frames != self.input_frames())
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return Err(PlaybackError::Media);
        }
        self.input = self
            .input
            .checked_add(frames as u64)
            .ok_or(PlaybackError::Time)?;
        let target = (u128::from(self.input) * u128::from(self.output_rate)
            + u128::from(self.input_rate / 2))
            / u128::from(self.input_rate);
        let target = u64::try_from(target).map_err(|_| PlaybackError::Time)?;
        let mut result = Vec::new();
        if self.converter.is_none() {
            result.extend(samples.chunks_exact(self.channels).map(stereo_frame));
        } else {
            self.convert(samples, frames, &mut result)?;
            while eof && self.output + (result.len() as u64) < target {
                self.convert(&[], 0, &mut result)?;
            }
            if eof {
                result.truncate(
                    usize::try_from(target.saturating_sub(self.output))
                        .map_err(|_| PlaybackError::Time)?,
                );
            }
        }
        self.output += result.len() as u64;
        self.done = eof;
        Ok(result)
    }

    fn convert(
        &mut self,
        samples: &[f32],
        frames: usize,
        result: &mut Vec<[f32; 2]>,
    ) -> Result<(), PlaybackError> {
        let converter = self.converter.as_mut().ok_or(PlaybackError::Media)?;
        let count = converter.output_frames_next();
        let mut output = vec![0.0; count * self.channels];
        let input = InterleavedSlice::new(samples, self.channels, frames)
            .map_err(|_| PlaybackError::Media)?;
        let mut view = InterleavedSlice::new_mut(&mut output, self.channels, count)
            .map_err(|_| PlaybackError::Media)?;
        let indexing = Indexing {
            partial_len: Some(frames),
            ..Indexing::default()
        };
        let (_, written) = converter
            .process_into_buffer(&input, &mut view, Some(&indexing))
            .map_err(|_| PlaybackError::Media)?;
        let skip = self.trim.min(written);
        self.trim -= skip;
        result.extend(
            output
                .chunks_exact(self.channels)
                .take(written)
                .skip(skip)
                .map(stereo_frame),
        );
        Ok(())
    }
}

/// The first two channels of `frame`. A mono frame plays on both.
fn stereo_frame(frame: &[f32]) -> [f32; 2] {
    let left = frame.first().copied().unwrap_or(0.0);
    [left, frame.get(1).copied().unwrap_or(left)]
}

#[cfg(test)]
#[path = "resample_stream_tests.rs"]
mod tests;
