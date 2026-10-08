//! Aliasing measurement for sample-rate converters.
//!
//! The cases and the window match ADR 0014, so a later candidate can be
//! compared with the same method:
//!
//! ```sh
//! cargo run --release -p echode-engine --bin src-measure
//! ```

use echode_engine::{ConverterMode, resample_with};
use std::f64::consts::PI;
use std::process::ExitCode;

fn main() -> ExitCode {
    let seconds = 1.0;
    for mode in [ConverterMode::Fft, ConverterMode::Sinc] {
        let worst = [
            alias_db(mode, 48_000, 44_100, 23_000.0, seconds),
            alias_db(mode, 96_000, 44_100, 30_000.0, seconds),
            alias_db(mode, 96_000, 48_000, 40_000.0, seconds),
            alias_db(mode, 44_100, 96_000, 20_000.0, seconds),
        ]
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max);
        let audible = alias_db(mode, 96_000, 44_100, 30_000.0, seconds);
        println!("{mode:?}\tworst {worst:.1} dB\taudible {audible:.1} dB");
    }
    ExitCode::SUCCESS
}

fn alias_db(
    mode: ConverterMode,
    input_rate: u32,
    output_rate: u32,
    tone_hz: f64,
    seconds: f64,
) -> f64 {
    let input_len = (f64::from(input_rate) * seconds).round() as usize;
    let mut input = vec![0.0f32; input_len];
    for (index, sample) in input.iter_mut().enumerate() {
        let phase = 2.0 * PI * tone_hz * index as f64 / f64::from(input_rate);
        *sample = phase.sin() as f32;
    }
    let output_len = (f64::from(output_rate) * seconds).round() as usize;
    let mut output = vec![0.0f32; output_len];
    resample_with(mode, &input, input_rate, &mut output, output_rate).expect("resample");
    let start = output_len / 4;
    let end = start + output_len / 2;
    blackman_harris(&mut output[start..end]);
    let alias = folded_alias(tone_hz, input_rate, output_rate);
    let alias_power = goertzel(&output[start..end], alias, output_rate);
    10.0 * (alias_power / reference_power(end - start, output_rate)).log10()
}

/// Where a tone above the output Nyquist, or an imaging replica, lands.
fn folded_alias(tone_hz: f64, input_rate: u32, output_rate: u32) -> f64 {
    let replica = if output_rate > input_rate {
        f64::from(input_rate) - tone_hz
    } else {
        tone_hz
    };
    let nyquist = f64::from(output_rate) / 2.0;
    let mut folded = replica.rem_euclid(f64::from(output_rate));
    if folded > nyquist {
        folded = f64::from(output_rate) - folded;
    }
    folded
}

fn reference_power(length: usize, sample_rate: u32) -> f64 {
    let mut reference = vec![0.0f32; length];
    for (index, sample) in reference.iter_mut().enumerate() {
        let phase = 2.0 * PI * 1_000.0 * index as f64 / f64::from(sample_rate);
        *sample = phase.sin() as f32;
    }
    blackman_harris(&mut reference);
    goertzel(&reference, 1_000.0, sample_rate)
}

fn blackman_harris(samples: &mut [f32]) {
    let n = samples.len() as f64;
    for (index, sample) in samples.iter_mut().enumerate() {
        let phase = 2.0 * PI * index as f64 / n;
        let window = 0.35875 - 0.48829 * phase.cos() + 0.14128 * (2.0 * phase).cos()
            - 0.01168 * (3.0 * phase).cos();
        *sample *= window as f32;
    }
}

fn goertzel(samples: &[f32], frequency: f64, sample_rate: u32) -> f64 {
    let n = samples.len() as f64;
    let k = (frequency / f64::from(sample_rate) * n).round();
    let coefficient = 2.0 * (2.0 * PI * k / n).cos();
    let mut first = 0.0;
    let mut second = 0.0;
    for sample in samples {
        let next = f64::from(*sample) + coefficient * first - second;
        second = first;
        first = next;
    }
    let power = first * first + second * second - coefficient * first * second;
    power.max(1e-30)
}
