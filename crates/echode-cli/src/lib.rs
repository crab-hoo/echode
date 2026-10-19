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

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

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
    durability_unknown: bool,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            durability_unknown: false,
        }
    }

    /// The WAV was renamed into place, then the parent directory could not be
    /// synced. The finished file is left in place.
    fn durability_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            durability_unknown: true,
        }
    }

    /// The finished file is on disk, but the directory sync after rename failed.
    pub fn is_durability_unknown(&self) -> bool {
        self.durability_unknown
    }
}

/// Next WAV step to fail. Cleared when that step runs.
#[doc(hidden)]
pub const WAV_FAULT_WRITE: u8 = 1;
#[doc(hidden)]
pub const WAV_FAULT_FLUSH: u8 = 2;
#[doc(hidden)]
pub const WAV_FAULT_RENAME: u8 = 3;
#[doc(hidden)]
pub const WAV_FAULT_DIR_SYNC: u8 = 4;

static WAV_FAULT: AtomicU8 = AtomicU8::new(0);
static WAV_FAULT_SKIP: AtomicU32 = AtomicU32::new(0);
static WAV_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
static EXPORT_LINES: AtomicU64 = AtomicU64::new(0);

/// Fails one WAV publish step after `skip` successful occurrences of that step.
///
/// Tests use this to inject a write, flush, rename, or directory-sync failure
/// without truncating an existing export.
#[doc(hidden)]
pub fn arm_wav_fault(kind: u8, skip: u32) {
    WAV_FAULT_SKIP.store(skip, Ordering::SeqCst);
    WAV_FAULT.store(kind, Ordering::SeqCst);
}

/// How many `exported` lines this process has written.
#[doc(hidden)]
pub fn export_lines_for_test() -> u64 {
    EXPORT_LINES.load(Ordering::Relaxed)
}

fn consume_wav_fault(kind: u8) -> bool {
    if WAV_FAULT.load(Ordering::SeqCst) != kind {
        return false;
    }
    let skip = WAV_FAULT_SKIP.load(Ordering::SeqCst);
    if skip > 0 {
        WAV_FAULT_SKIP.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    WAV_FAULT.store(0, Ordering::SeqCst);
    true
}

fn note_export(revision: &str, seq: u64) {
    eprintln!("exported {revision} {seq}");
    EXPORT_LINES.fetch_add(1, Ordering::Relaxed);
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
        "mcp" => mcp_command(&mut args),
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

fn mcp_command<I, S>(args: &mut I) -> Result<(), CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let Some(socket) = args.next() else {
        return Err(usage());
    };
    if args.next().is_some() {
        return Err(usage());
    }
    echode_api::serve_mcp(
        Path::new(socket.as_ref()),
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
    .map_err(|error| CliError::new(error.to_string()))
}

/// Recovers a package and writes a stereo IEEE-float WAV.
///
/// The project is the document host's snapshot. On success, after the WAV
/// exists, stderr receives `exported <autosave-id> <journal-seq>`. The id is
/// the base the journal was replayed onto. The sequence is the last journal
/// line included in that snapshot.
pub fn render_to_wav(
    package: &Path,
    wav: &Path,
    sample_rate: u32,
    frames: Option<u32>,
) -> Result<(), CliError> {
    let _activity = echode_audio_io::UserActivity::begin("Echode is writing audio");
    let host = DocumentHost::open(package).map_err(|error| CliError::new(error.to_string()))?;
    let project = host.snapshot();
    let (revision, seq) = host
        .recovered_label()
        .map_err(|error| CliError::new(error.to_string()))?;
    drop(host);
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
        wav.finish()?;
        note_export(&revision, seq);
        return Ok(());
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
    render_with_plugins(package, wav, &project, sample_rate, frames, &host, &routes)?;
    note_export(&revision, seq);
    Ok(())
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
    let mut wav = WavFile::create(wav, sample_rate, frames)?;
    let mut left = vec![0.0; block];
    let mut right = vec![0.0; block];
    let mut interleaved = vec![0.0; block.saturating_mul(2)];
    let mut rendered = 0usize;
    while rendered < total {
        let count = (total - rendered).min(block);
        audio.render(&mut left[..count], &mut right[..count]);
        if let Some(reason) = control.plugin_failure() {
            return Err(CliError::new(reason));
        }
        if control.status().failed {
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
        .map_err(|_| CliError::new(format!("expected a non-negative integer, got {text}")))
}

fn usage() -> CliError {
    CliError::new(
        "usage: echode render PACKAGE WAV [--sample-rate 48000] [--frames N]\n       echode serve PACKAGE SOCKET\n       echode mcp SOCKET",
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
    temporary: PathBuf,
    destination: PathBuf,
    /// Set once `rename` has replaced the destination. Drop must not delete it.
    published: bool,
}

impl Drop for WavFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

impl WavFile {
    fn create(path: &Path, sample_rate: u32, frames: u32) -> Result<Self, CliError> {
        let header = wav_header(sample_rate, frames)?;
        let destination = path.to_path_buf();
        let temporary = temporary_wav_path(&destination)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| CliError::new(error.to_string()))?;
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        if let Err(error) = writer.write_all(&header) {
            drop(writer);
            let _ = std::fs::remove_file(&temporary);
            return Err(CliError::new(error.to_string()));
        }
        Ok(Self {
            writer,
            bytes: Vec::with_capacity(64 * 2 * 4),
            temporary,
            destination,
            published: false,
        })
    }

    fn write_block(&mut self, interleaved: &[f32]) -> Result<(), CliError> {
        if consume_wav_fault(WAV_FAULT_WRITE) {
            return Err(CliError::new("wav write failed"));
        }
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
        self.publish()
    }

    /// Header, PCM, flush, and file sync succeed before the same-directory rename.
    fn publish(&mut self) -> Result<(), CliError> {
        if consume_wav_fault(WAV_FAULT_FLUSH) {
            return Err(CliError::new("wav flush failed"));
        }
        self.writer
            .flush()
            .map_err(|error| CliError::new(error.to_string()))?;
        self.writer
            .get_ref()
            .sync_all()
            .map_err(|error| CliError::new(error.to_string()))?;
        if consume_wav_fault(WAV_FAULT_RENAME) {
            return Err(CliError::new("wav rename failed"));
        }
        std::fs::rename(&self.temporary, &self.destination)
            .map_err(|error| CliError::new(error.to_string()))?;
        self.published = true;
        if consume_wav_fault(WAV_FAULT_DIR_SYNC) {
            return Err(CliError::durability_unknown("export durability is unknown"));
        }
        sync_parent(&self.destination).map_err(|error| {
            CliError::durability_unknown(format!("export durability is unknown: {error}"))
        })
    }
}

fn wav_header(sample_rate: u32, frames: u32) -> Result<[u8; 44], CliError> {
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
    Ok(header)
}

fn temporary_wav_path(destination: &Path) -> Result<PathBuf, CliError> {
    let name = destination
        .file_name()
        .ok_or_else(|| CliError::new("wav path has no file name"))?;
    let parent = destination.parent().unwrap_or(Path::new(""));
    let seq = WAV_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut temporary = parent.to_path_buf();
    temporary.push(format!(
        ".{}.{}.{seq}.part",
        name.to_string_lossy(),
        std::process::id()
    ));
    Ok(temporary)
}

fn sync_parent(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()
}
