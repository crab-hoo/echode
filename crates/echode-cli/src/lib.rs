//! `echode render` writes a project package to a stereo float WAV.
//!
//! A project with no CLAP device uses the test synth. Placed audio is loaded
//! the same way as product playback and mixed after the synth. A project with
//! a CLAP device uses that same isolated playback chain, and refuses the
//! export when a plugin or the helper is missing. The project is the document
//! host's recovered snapshot, so a journaled edit is included and a save that
//! is still rewriting the worktree is not mixed in. Each render block is written
//! as it is produced, through a buffered file, so a long export is not one
//! syscall per sample and not one buffer for the whole file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use echode_api::serve;
use echode_audio_io::{load_placed_clips, project_plugin_ids, routes_with_saved_state};
use echode_engine::{
    PlaybackCommand, PluginRoute, playback_with_plugins, render_offline_blocks_with_clips,
    render_span_frames,
};
use echode_history::DocumentHost;
use echode_model::Project;

/// One product callback while exporting plugins. Scratch for the helper, not a
/// limit on the export. 256 frames at 48 kHz is about 5.3 ms.
const PLUGIN_CALLBACK_FRAMES: usize = 256;

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

/// Recovers a package and writes a stereo IEEE-float WAV.
///
/// The project is the document host's snapshot. On success the autosave
/// revision that snapshot belongs to is written to stderr as `exported <id>`.
pub fn render_to_wav(
    package: &Path,
    wav: &Path,
    sample_rate: u32,
    frames: Option<u32>,
) -> Result<(), CliError> {
    let host = DocumentHost::open(package).map_err(|error| CliError::new(error.to_string()))?;
    let project = host.snapshot();
    let revision = host
        .revisions()
        .map_err(|error| CliError::new(error.to_string()))?
        .into_iter()
        .next()
        .map(|revision| revision.id);
    drop(host);
    if let Some(id) = revision {
        eprintln!("exported {id}");
    }
    let frames = match frames {
        Some(frames) => frames,
        None => render_span_frames(&project, sample_rate)
            .map_err(|error| CliError::new(error.to_string()))?,
    };
    let ids = project_plugin_ids(&project);
    if ids.is_empty() {
        let clips = load_placed_clips(package, &project, sample_rate)
            .map_err(|error| CliError::new(error.to_string()))?;
        let mut wav = WavFile::create(wav, sample_rate, frames)?;
        render_offline_blocks_with_clips(&project, sample_rate, frames, 64, &clips, |block| {
            wav.write_block(block)
        })
        .map_err(|error| CliError::new(error.to_string()))?;
        return wav.finish();
    }
    let Some(host) = echode_plugin_host::host_binary() else {
        return Err(CliError::new("plugin host is missing"));
    };
    let routes = routes_with_saved_state(&project, package)
        .map_err(|error| CliError::new(error.to_string()))?;
    let missing: Vec<&str> = ids
        .iter()
        .filter(|id| {
            !routes
                .iter()
                .any(|route| route.clap_id == **id && route.path.is_file())
        })
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(CliError::new(format!(
            "missing plugin: {}",
            missing.join(", ")
        )));
    }
    render_with_plugins(package, wav, &project, sample_rate, frames, &host, &routes)
}

fn render_with_plugins(
    package: &Path,
    wav: &Path,
    project: &Project,
    sample_rate: u32,
    frames: u32,
    host: &Path,
    routes: &[PluginRoute],
) -> Result<(), CliError> {
    if frames == 0 {
        return WavFile::create(wav, sample_rate, 0)?.finish();
    }
    let total = usize::try_from(frames).map_err(|_| CliError::new("export length overflowed"))?;
    let block = total.min(PLUGIN_CALLBACK_FRAMES);
    let (mut control, mut audio) = playback_with_plugins(project, sample_rate, block, host, routes)
        .map_err(|error| CliError::new(error.to_string()))?;
    let package = package.to_path_buf();
    control.set_clip_loader(Arc::new(move |project, rate| {
        load_placed_clips(&package, project, rate)
    }));
    control
        .replace(project)
        .map_err(|error| CliError::new(error.to_string()))?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|error| CliError::new(error.to_string()))?;
    let wav_path = wav.to_path_buf();
    let mut wav = WavFile::create(wav, sample_rate, frames)?;
    let mut left = vec![0.0; block];
    let mut right = vec![0.0; block];
    let mut interleaved = vec![0.0; block.saturating_mul(2)];
    let mut rendered = 0usize;
    while rendered < total {
        let count = (total - rendered).min(block);
        audio.render(&mut left[..count], &mut right[..count]);
        if let Some(reason) = control.plugin_failure() {
            drop(wav);
            let _ = std::fs::remove_file(wav_path);
            return Err(CliError::new(reason));
        }
        if control.status().failed {
            drop(wav);
            let _ = std::fs::remove_file(wav_path);
            return Err(CliError::new("plugin playback failed"));
        }
        for index in 0..count {
            let left = left.get(index).copied().unwrap_or(0.0);
            let right = right.get(index).copied().unwrap_or(0.0);
            if let Some(slot) = interleaved.get_mut(index * 2) {
                *slot = left;
            }
            if let Some(slot) = interleaved.get_mut(index * 2 + 1) {
                *slot = right;
            }
        }
        let samples = count.saturating_mul(2);
        wav.write_block(interleaved.get(..samples).unwrap_or(&[]))?;
        control.service();
        rendered += count;
    }
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
