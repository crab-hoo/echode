//! `echode render` writes a project package to a stereo float WAV.
//!
//! The samples come from the test synth. Two renders of the same package
//! write the same bytes.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use echode_api::serve;
use echode_engine::{render_offline, render_span_frames};
use echode_history::read_package;

/// Why a command stopped.
#[derive(Debug)]
pub struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// Runs one invocation. `args` does not include the program name.
pub fn run<I, S>(args: I) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(usage());
    };
    match command.as_ref() {
        "render" => render_command(&mut args),
        "serve" => serve_command(&mut args),
        _ => Err(usage()),
    }
}

fn render_command<I, S>(args: &mut I) -> Result<(), CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let Some(package) = args.next() else {
        return Err(usage());
    };
    let Some(wav) = args.next() else {
        return Err(usage());
    };
    let mut sample_rate = 48_000u32;
    let mut frames = None;
    while let Some(flag) = args.next() {
        match flag.as_ref() {
            "--sample-rate" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                sample_rate = parse_u32(value.as_ref())?;
            }
            "--frames" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                frames = Some(parse_u32(value.as_ref())?);
            }
            _ => return Err(usage()),
        }
    }
    render_to_wav(
        Path::new(package.as_ref()),
        Path::new(wav.as_ref()),
        sample_rate,
        frames,
    )
}

fn serve_command<I, S>(args: &mut I) -> Result<(), CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let Some(package) = args.next() else {
        return Err(usage());
    };
    let Some(socket) = args.next() else {
        return Err(usage());
    };
    if args.next().is_some() {
        return Err(usage());
    }
    let project = read_package(Path::new(package.as_ref()))
        .map_err(|error| CliError::new(error.to_string()))?;
    serve(project, Path::new(socket.as_ref())).map_err(|error| CliError::new(error.to_string()))
}

/// Reads a package and writes a stereo IEEE-float WAV.
pub fn render_to_wav(
    package: &Path,
    wav: &Path,
    sample_rate: u32,
    frames: Option<u32>,
) -> Result<(), CliError> {
    let project = read_package(package).map_err(|error| CliError::new(error.to_string()))?;
    let frames = match frames {
        Some(frames) => frames,
        None => render_span_frames(&project, sample_rate)
            .map_err(|error| CliError::new(error.to_string()))?,
    };
    let mut interleaved = vec![0.0; usize::try_from(frames).unwrap_or(0).saturating_mul(2)];
    render_offline(&project, sample_rate, frames, &mut interleaved)
        .map_err(|error| CliError::new(error.to_string()))?;
    write_wav(wav, sample_rate, &interleaved)
}

fn parse_u32(text: &str) -> Result<u32, CliError> {
    text.parse::<u32>()
        .map_err(|_| CliError::new(format!("expected a positive integer, got {text}")))
}

fn usage() -> CliError {
    CliError::new(
        "usage: echode render PACKAGE WAV [--sample-rate 48000] [--frames N]\n       echode serve PACKAGE SOCKET",
    )
}

/// Canonical stereo 32-bit float WAV. The data chunk must fit in 4 GiB.
pub fn write_wav(path: &Path, sample_rate: u32, interleaved: &[f32]) -> Result<(), CliError> {
    let data_bytes = u32::try_from(interleaved.len().saturating_mul(4))
        .map_err(|_| CliError::new("wav data does not fit in a 32-bit chunk"))?;
    let riff_size = data_bytes
        .checked_add(36)
        .ok_or_else(|| CliError::new("wav data does not fit in a 32-bit chunk"))?;
    let byte_rate = sample_rate
        .checked_mul(8)
        .ok_or_else(|| CliError::new("sample rate does not fit in the wav header"))?;
    let mut file = File::create(path).map_err(|error| CliError::new(error.to_string()))?;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&riff_size.to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&3u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&8u16.to_le_bytes());
    header.extend_from_slice(&32u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    file.write_all(&header)
        .and_then(|_| {
            for sample in interleaved {
                file.write_all(&sample.to_le_bytes())?;
            }
            Ok(())
        })
        .map_err(|error| CliError::new(error.to_string()))
}
