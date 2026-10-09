//! `echode render` writes a project package to a stereo float WAV.
//!
//! The samples come from the test synth. Each render block is written as it
//! is produced, through a buffered file, so a long export is not one syscall
//! per sample and not one buffer for the whole file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use echode_api::serve;
use echode_engine::{render_offline_blocks, render_span_frames};
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
    serve(Path::new(package.as_ref()), Path::new(socket.as_ref()))
        .map_err(|error| CliError::new(error.to_string()))
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
    let mut wav = WavFile::create(wav, sample_rate, frames)?;
    render_offline_blocks(&project, sample_rate, frames, 64, |block| {
        wav.write_block(block)
    })
    .map_err(|error| CliError::new(error.to_string()))?;
    wav.finish()
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
///
/// Samples are converted in groups and written through [`BufWriter`].
pub fn write_wav(path: &Path, sample_rate: u32, interleaved: &[f32]) -> Result<(), CliError> {
    if !interleaved.len().is_multiple_of(2) {
        return Err(CliError::new("wav data must be stereo"));
    }
    let frames = u32::try_from(interleaved.len() / 2)
        .map_err(|_| CliError::new("wav data does not fit in a 32-bit chunk"))?;
    let mut wav = WavFile::create(path, sample_rate, frames)?;
    for block in interleaved.chunks(64 * 2) {
        wav.write_block(block)?;
    }
    wav.finish()
}

struct WavFile {
    writer: BufWriter<File>,
    bytes: Vec<u8>,
}

impl WavFile {
    fn create(path: &Path, sample_rate: u32, frames: u32) -> Result<Self, CliError> {
        let data_bytes = frames
            .checked_mul(8)
            .ok_or_else(|| CliError::new("wav data does not fit in a 32-bit chunk"))?;
        let riff_size = data_bytes
            .checked_add(36)
            .ok_or_else(|| CliError::new("wav data does not fit in a 32-bit chunk"))?;
        let byte_rate = sample_rate
            .checked_mul(8)
            .ok_or_else(|| CliError::new("sample rate does not fit in the wav header"))?;
        let mut header = [0u8; 44];
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&riff_size.to_le_bytes());
        header[8..12].copy_from_slice(b"WAVE");
        header[12..16].copy_from_slice(b"fmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes());
        header[20..22].copy_from_slice(&3u16.to_le_bytes());
        header[22..24].copy_from_slice(&2u16.to_le_bytes());
        header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
        header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        header[32..34].copy_from_slice(&8u16.to_le_bytes());
        header[34..36].copy_from_slice(&32u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
        let file = File::create(path).map_err(|error| CliError::new(error.to_string()))?;
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        writer
            .write_all(&header)
            .map_err(|error| CliError::new(error.to_string()))?;
        Ok(Self {
            writer,
            bytes: Vec::with_capacity(64 * 2 * 4),
        })
    }

    fn write_block(&mut self, interleaved: &[f32]) -> Result<(), CliError> {
        self.bytes.clear();
        self.bytes.reserve(interleaved.len().saturating_mul(4));
        for sample in interleaved {
            self.bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.writer
            .write_all(&self.bytes)
            .map_err(|error| CliError::new(error.to_string()))
    }

    fn finish(mut self) -> Result<(), CliError> {
        self.writer
            .flush()
            .map_err(|error| CliError::new(error.to_string()))
    }
}
