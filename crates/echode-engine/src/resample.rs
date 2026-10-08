//! Sample-rate conversion through rubato 5.0.1.
//!
//! A constant ratio uses the FFT resampler. A ratio that can change uses the
//! sinc resampler. Both are rubato's default settings. The same rate does not
//! enter the converter: samples are copied, and anything past the input is
//! silence. Rubato's startup delay is trimmed so output sample 0 matches input
//! sample 0. The process call itself does not allocate; construction may.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, Fft, FixedAsync, FixedSync, Resampler, SincInterpolationParameters};

use crate::realtime::ForbidAlloc;
use crate::transport::PlaybackError;

/// Identity recorded in a project and in a preview-cache key.
pub const CONVERTER_ID: &str = "rubato";
/// The version the aliasing figures in ADR 0014 were measured with.
pub const CONVERTER_VERSION: &str = "5.0.1";

/// Which rubato algorithm a clip uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConverterMode {
    /// Fixed ratio. Rubato's FFT resampler.
    Fft,
    /// The ratio may change while the clip plays. Rubato's sinc resampler.
    Sinc,
}

/// Playback and export start on the same algorithm. Each setting can diverge later.
pub fn default_converter() -> ConverterMode {
    ConverterMode::Fft
}

/// Converts `input` into `output`.
///
/// `output[0]` is the sample at the same instant as `input[0]`. Samples after
/// the input has ended are silence.
pub fn resample_into(
    input: &[f32],
    input_rate: u32,
    output: &mut [f32],
    output_rate: u32,
) -> Result<(), PlaybackError> {
    resample_with(ConverterMode::Fft, input, input_rate, output, output_rate)
}

/// Same contract as [`resample_into`], selecting the FFT or sinc algorithm.
pub fn resample_with(
    mode: ConverterMode,
    input: &[f32],
    input_rate: u32,
    output: &mut [f32],
    output_rate: u32,
) -> Result<(), PlaybackError> {
    if input_rate == 0 || output_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    if input_rate == output_rate {
        let _guard = ForbidAlloc::enter();
        copy_same_rate(input, output);
        return Ok(());
    }
    if input.is_empty() {
        output.fill(0.0);
        return Ok(());
    }
    match mode {
        ConverterMode::Fft => convert_fft(input, input_rate, output, output_rate),
        ConverterMode::Sinc => convert_sinc(input, input_rate, output, output_rate),
    }
}

fn copy_same_rate(input: &[f32], output: &mut [f32]) {
    for (index, sample) in output.iter_mut().enumerate() {
        *sample = input.get(index).copied().unwrap_or(0.0);
    }
}

fn convert_fft(
    input: &[f32],
    input_rate: u32,
    output: &mut [f32],
    output_rate: u32,
) -> Result<(), PlaybackError> {
    // 1024 frames is the processing chunk, not a limit on clip length.
    let mut resampler = Fft::<f32>::new(
        input_rate as usize,
        output_rate as usize,
        1024,
        1,
        FixedSync::Both,
    )
    .map_err(|_| PlaybackError::InvalidSampleRate)?;
    convert_all(&mut resampler, input, output)
}

fn convert_sinc(
    input: &[f32],
    input_rate: u32,
    output: &mut [f32],
    output_rate: u32,
) -> Result<(), PlaybackError> {
    let ratio = f64::from(output_rate) / f64::from(input_rate);
    let mut resampler = Async::<f32>::new_sinc(
        ratio,
        2.0,
        &SincInterpolationParameters::default(),
        1024,
        1,
        FixedAsync::Input,
    )
    .map_err(|_| PlaybackError::InvalidSampleRate)?;
    convert_all(&mut resampler, input, output)
}

fn convert_all<R>(resampler: &mut R, input: &[f32], output: &mut [f32]) -> Result<(), PlaybackError>
where
    R: Resampler<f32>,
{
    let needed = resampler
        .process_all_needed_output_len(input.len())
        .max(output.len());
    let mut produced = vec![0.0f32; needed];
    let input_view =
        InterleavedSlice::new(input, 1, input.len()).map_err(|_| PlaybackError::Time)?;
    let mut output_view =
        InterleavedSlice::new_mut(&mut produced, 1, needed).map_err(|_| PlaybackError::Time)?;
    let written = {
        let _guard = ForbidAlloc::enter();
        let (_consumed, frames) = resampler
            .process_all_into_buffer(&input_view, &mut output_view, input.len(), None)
            .map_err(|_| PlaybackError::Time)?;
        frames
    };
    for (index, sample) in output.iter_mut().enumerate() {
        *sample = if index < written {
            produced[index]
        } else {
            0.0
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rates_pass_samples_through_without_the_converter() {
        let mut input = [0.0; 128];
        input[64] = 1.0;
        let mut output = [0.0; 128];
        resample_into(&input, 48_000, &mut output, 48_000).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn a_constant_keeps_its_level_when_the_rate_changes() {
        let input = [0.5f32; 4096];
        let mut up = [0.0; 4096];
        resample_into(&input, 44_100, &mut up, 48_000).unwrap();
        assert!((up[2048] - 0.5).abs() < 1e-3, "fft up {}", up[2048]);

        let mut down = [0.0; 2048];
        resample_into(&input, 48_000, &mut down, 22_050).unwrap();
        assert!((down[1024] - 0.5).abs() < 1e-3, "fft down {}", down[1024]);

        let mut sinc = [0.0; 4096];
        resample_with(ConverterMode::Sinc, &input, 44_100, &mut sinc, 48_000).unwrap();
        assert!((sinc[2048] - 0.5).abs() < 1e-3, "sinc {}", sinc[2048]);
    }

    #[test]
    fn the_first_output_sample_lines_up_with_the_first_input_sample() {
        let mut input = [0.0; 4096];
        input[0] = 1.0;
        let mut output = [0.0; 8192];
        resample_into(&input, 24_000, &mut output, 48_000).unwrap();
        let peak = output
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .unwrap()
            .0;
        assert!(peak < 8, "startup delay was not removed, peak at {peak}");
    }

    #[test]
    fn samples_past_the_input_are_silence() {
        let input = [0.5f32; 64];
        let mut output = [1.0; 8000];
        resample_into(&input, 48_000, &mut output, 48_000).unwrap();
        assert!(output[4000..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_zero_rate_is_rejected() {
        let input = [1.0; 8];
        let mut output = [0.0; 8];
        assert_eq!(
            resample_into(&input, 0, &mut output, 48_000).unwrap_err(),
            PlaybackError::InvalidSampleRate
        );
        assert_eq!(default_converter(), ConverterMode::Fft);
        assert_eq!(CONVERTER_ID, "rubato");
        assert_eq!(CONVERTER_VERSION, "5.0.1");
    }
}
