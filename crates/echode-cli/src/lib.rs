//! `echode render` writes a project package to a stereo float, 16-bit PCM, or 24-bit PCM WAV.
//!
//! A project with no instrument does not synthesize notes. Placed audio uses
//! the same buffers and faders as product playback. A project with
//! a CLAP device uses that same isolated playback chain, and refuses the
//! export when a plugin or the helper is missing. The project is the document
//! host's recovered snapshot, so a journaled edit is included and a save that
//! is still rewriting the worktree is not mixed in. Render blocks are buffered,
//! not kept in one whole-song buffer. Optional peak normalization spools the
//! mixed float samples once, then applies one linked gain before WAV quantization.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use echode_api::serve;
use echode_audio_io::{load_placed_clips, project_plugin_ids, routes_with_saved_state};
use echode_engine::{
    PlaybackCommand, PlaybackOptions, PluginRoute, playback, render_offline_blocks_with_clips,
    render_span_frames,
};
use echode_history::DocumentHost;
use echode_model::Project;

mod destination;
use destination::RenderPaths;

mod cancellation;
use cancellation::Cancellation;

mod dither;
use dither::{Dither, Tpdf};
mod normalization;
use normalization::{ExportOutput, PeakTarget};

mod progress;
use progress::RenderProgress;

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;

/// One product callback while exporting plugins. Scratch for the helper, not a
/// limit on the export. 256 frames at 48 kHz is about 5.3 ms. The block does
/// not depend on the export's length or tail; the last one is shorter.
const PLUGIN_CALLBACK_FRAMES: usize = 256;

/// How long an export waits for its helpers to finish one block. An export is
/// not real time, so a plugin slower than real time still finishes (CLI-A1).
/// This only ends an export whose helper stopped answering.
const OFFLINE_BLOCK_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
enum WavFormat {
    Float32,
    Pcm16,
    Pcm24,
}

impl WavFormat {
    fn parse(text: &str) -> Result<Self, CliError> {
        match text {
            "float32" => Ok(Self::Float32),
            "pcm16" => Ok(Self::Pcm16),
            "pcm24" => Ok(Self::Pcm24),
            _ => Err(ArgumentError::WavFormat(text.into()).into()),
        }
    }

    fn frame_bytes(self) -> u16 {
        match self {
            Self::Float32 => 8,
            Self::Pcm16 => 4,
            Self::Pcm24 => 6,
        }
    }
}

mod error;
pub use error::{ArgumentError, CliError, CliIoOperation};

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
    let mut tail_frames = None;
    let mut format = WavFormat::Float32;
    let mut dither = Dither::None;
    let mut peak_target = None;
    let mut progress = RenderProgress::new(false);
    let mut cancel_stdin = false;
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
            "--tail-frames" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                tail_frames = Some(parse_u32(value.as_ref())?);
            }
            "--format" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                format = WavFormat::parse(value.as_ref())?;
            }
            "--dither" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                dither = Dither::parse(value.as_ref())?;
            }
            "--normalize-peak-db" => {
                let Some(value) = args.next() else {
                    return Err(usage());
                };
                peak_target = Some(PeakTarget::parse(value.as_ref())?);
            }
            "--progress-json" => progress = RenderProgress::new(true),
            "--cancel-stdin" => cancel_stdin = true,
            _ => return Err(usage()),
        }
    }
    let cancellation = if cancel_stdin {
        Cancellation::from_stdin()?
    } else {
        Cancellation::default()
    };
    render_to_wav_format(
        Path::new(package.as_ref()),
        Path::new(wav.as_ref()),
        sample_rate,
        frames,
        tail_frames,
        format,
        dither,
        peak_target,
        &mut progress,
        &cancellation,
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
    serve(Path::new(package.as_ref()), Path::new(socket.as_ref())).map_err(CliError::from)
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
    .map_err(CliError::from)
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
    render_to_wav_format(
        package,
        wav,
        sample_rate,
        frames,
        None,
        WavFormat::Float32,
        Dither::None,
        None,
        &mut RenderProgress::new(false),
        &Cancellation::default(),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_to_wav_format(
    package: &Path,
    wav: &Path,
    sample_rate: u32,
    frames: Option<u32>,
    tail_frames: Option<u32>,
    format: WavFormat,
    dither: Dither,
    peak_target: Option<PeakTarget>,
    progress: &mut RenderProgress,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    cancellation.check()?;
    dither.validate(format)?;
    if sample_rate == 0 {
        return Err(ArgumentError::SampleRate.into());
    }
    if frames.is_some() && tail_frames.is_some() {
        return Err(ArgumentError::ConflictingLength.into());
    }
    let tail = if frames.is_some() {
        RenderTail::Frames(0)
    } else {
        tail_frames.map_or(RenderTail::Automatic, RenderTail::Frames)
    };
    let paths = RenderPaths::resolve(package, wav)?;
    let package = paths.package.as_path();
    let wav = paths.output.as_path();
    let _activity = echode_audio_io::UserActivity::begin("Echode is writing audio");
    let host = DocumentHost::open(package).map_err(CliError::from)?;
    let project = host.snapshot();
    let (revision, seq) = host.recovered_label().map_err(CliError::from)?;
    drop(host);
    let frames = match frames {
        Some(frames) => frames,
        None => render_span_frames(&project, sample_rate).map_err(CliError::from)?,
    };
    // Validate explicit length overflow before loading media or starting helpers.
    let minimum_frames = tail.total_frames(frames, 0)?;
    let ids = project_plugin_ids(&project);
    if ids.is_empty() {
        let clips = load_placed_clips(package, &project, sample_rate).map_err(CliError::from)?;
        cancellation.check()?;
        let mut wav = ExportOutput::create(
            wav,
            sample_rate,
            minimum_frames,
            format,
            dither,
            peak_target,
        )?;
        progress.start(minimum_frames);
        render_offline_blocks_with_clips(
            &project,
            sample_rate,
            minimum_frames,
            64,
            &clips,
            |block| {
                cancellation.check()?;
                wav.write_block(block)?;
                progress.advance((block.len() / 2) as u32);
                Ok::<_, CliError>(())
            },
        )
        .map_err(|error| match error {
            echode_engine::OfflineError::Write(error) => error,
            echode_engine::OfflineError::Playback(error) => CliError::Playback(error),
        })?;
        progress.finalizing();
        wav.finish_cancellable(cancellation)?;
        progress.complete();
        note_export(&revision, seq);
        return Ok(());
    }
    let Some(host) = echode_plugin_host::host_binary() else {
        return Err(CliError::MissingPluginHost);
    };
    let routes = routes_with_saved_state(&project, package);
    let missing: Vec<&str> = ids
        .iter()
        .filter(|id| {
            !routes
                .iter()
                .any(|route| route.clap_id == **id && route.path.exists())
        })
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(CliError::MissingPlugins(
            missing.into_iter().map(str::to_owned).collect(),
        ));
    }
    render_with_plugins(
        package,
        wav,
        &project,
        sample_rate,
        frames,
        tail,
        format,
        dither,
        peak_target,
        &host,
        &routes,
        progress,
        cancellation,
    )?;
    progress.complete();
    note_export(&revision, seq);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_with_plugins(
    package: &Path,
    wav: &Path,
    project: &Project,
    sample_rate: u32,
    frames: u32,
    tail: RenderTail,
    format: WavFormat,
    dither: Dither,
    peak_target: Option<PeakTarget>,
    host: &Path,
    routes: &[PluginRoute],
    progress: &mut RenderProgress,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    cancellation.check()?;
    if frames == 0 && matches!(tail, RenderTail::Automatic | RenderTail::Frames(0)) {
        let wav = ExportOutput::create(wav, sample_rate, 0, format, dither, peak_target)?;
        progress.start(0);
        progress.finalizing();
        return wav.finish_cancellable(cancellation);
    }
    let block = PLUGIN_CALLBACK_FRAMES;
    let (mut control, mut audio) = playback(
        project,
        sample_rate,
        block,
        PlaybackOptions::default().plugins(host, routes),
    )
    .map_err(CliError::from)?;
    let frames = tail.total_frames(frames, control.tail_samples())?;
    let total = frames as usize;
    let package = package.to_path_buf();
    control.set_clip_loader(Arc::new(move |project, rate| {
        load_placed_clips(&package, project, rate)
    }));
    control.replace(project).map_err(CliError::from)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(CliError::from)?;
    cancellation.check()?;
    let mut wav = ExportOutput::create(wav, sample_rate, frames, format, dither, peak_target)?;
    progress.start(frames);
    let mut left = vec![0.0; block];
    let mut right = vec![0.0; block];
    let mut interleaved = vec![0.0; block.saturating_mul(2)];
    let mut rendered = 0usize;
    while rendered < total {
        cancellation.check()?;
        let count = (total - rendered).min(block);
        audio.render_until(
            &mut left[..count],
            &mut right[..count],
            Instant::now() + OFFLINE_BLOCK_WAIT,
        );
        cancellation.check()?;
        if let Some(reason) = control.plugin_failure() {
            return Err(CliError::PluginBlock(reason));
        }
        if control.status().failed {
            return Err(CliError::PluginPlaybackFailed);
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
        progress.advance(count as u32);
    }
    progress.finalizing();
    wav.finish_cancellable(cancellation)
}

#[derive(Clone, Copy)]
enum RenderTail {
    Automatic,
    Frames(u32),
}

impl RenderTail {
    fn total_frames(self, frames: u32, device_tail: u32) -> Result<u32, CliError> {
        let tail = match self {
            Self::Automatic => {
                if device_tail >= i32::MAX as u32 {
                    return Err(CliError::InfiniteTail);
                }
                device_tail
            }
            Self::Frames(tail) => tail,
        };
        frames
            .checked_add(tail)
            .ok_or(CliError::ExportLengthOverflow)
    }
}

fn parse_u32(text: &str) -> Result<u32, CliError> {
    text.parse::<u32>().map_err(|error| {
        ArgumentError::Integer {
            text: text.into(),
            error,
        }
        .into()
    })
}

fn usage() -> CliError {
    ArgumentError::Usage.into()
}

/// Stereo 32-bit IEEE float WAV with WAVEFORMATEX and a frame-count `fact` chunk.
///
/// Exports beyond RIFF's 4 GiB size field use RF64 with 64-bit chunk sizes.
/// Samples are converted in groups and written through [`BufWriter`].
pub fn write_wav(path: &Path, sample_rate: u32, interleaved: &[f32]) -> Result<(), CliError> {
    if !interleaved.len().is_multiple_of(2) {
        return Err(CliError::WavStereo);
    }
    let frames = u32::try_from(interleaved.len() / 2).map_err(CliError::WavFrameCount)?;
    let mut wav = WavFile::create(path, sample_rate, frames, WavFormat::Float32)?;
    for block in interleaved.chunks(64 * 2) {
        wav.write_block(block)?;
    }
    wav.finish()
}

struct WavFile {
    writer: BufWriter<File>,
    bytes: Vec<u8>,
    format: WavFormat,
    dither: Option<Tpdf>,
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
    fn create(
        path: &Path,
        sample_rate: u32,
        frames: u32,
        format: WavFormat,
    ) -> Result<Self, CliError> {
        Self::create_with_dither(path, sample_rate, frames, format, Dither::None)
    }

    fn create_with_dither(
        path: &Path,
        sample_rate: u32,
        frames: u32,
        format: WavFormat,
        dither: Dither,
    ) -> Result<Self, CliError> {
        dither.validate(format)?;
        let header = wav_header(sample_rate, frames, format)?;
        let destination = path.to_path_buf();
        let temporary = temporary_wav_path(&destination)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| CliError::io(CliIoOperation::CreateWav, temporary.clone(), error))?;
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        if let Err(error) = writer.write_all(&header) {
            drop(writer);
            let _ = std::fs::remove_file(&temporary);
            return Err(CliError::io(
                CliIoOperation::WriteWavHeader,
                temporary,
                error,
            ));
        }
        Ok(Self {
            writer,
            bytes: Vec::with_capacity(64 * 2 * 4),
            format,
            dither: dither.generator(),
            temporary,
            destination,
            published: false,
        })
    }

    fn write_block(&mut self, interleaved: &[f32]) -> Result<(), CliError> {
        if consume_wav_fault(WAV_FAULT_WRITE) {
            return Err(CliError::WavWriteFailed);
        }
        self.bytes.clear();
        self.bytes.reserve(
            interleaved
                .len()
                .saturating_mul(usize::from(self.format.frame_bytes() / 2)),
        );
        for sample in interleaved {
            match self.format {
                WavFormat::Float32 => self.bytes.extend_from_slice(&sample.to_le_bytes()),
                WavFormat::Pcm16 | WavFormat::Pcm24 => {
                    let bits = self.format.frame_bytes() * 4;
                    if !sample.is_finite() {
                        return Err(CliError::NonFinitePcm { bits });
                    }
                    // Quantize once, at the file boundary. No noise shaping.
                    // f64 preserves the half-LSB rounding boundary near full scale.
                    let scale = f64::from(1u32 << (bits - 1));
                    let noise = self.dither.as_mut().map_or(0.0, Tpdf::sample_lsb);
                    let integer = (f64::from(*sample) * scale + noise)
                        .round()
                        .clamp(-scale, scale - 1.0) as i32;
                    self.bytes
                        .extend_from_slice(&integer.to_le_bytes()[..usize::from(bits / 8)]);
                }
            }
        }
        self.writer.write_all(&self.bytes).map_err(|error| {
            CliError::io(
                CliIoOperation::WriteWavSamples,
                self.temporary.clone(),
                error,
            )
        })
    }

    fn finish(self) -> Result<(), CliError> {
        self.finish_cancellable(&Cancellation::default())
    }

    fn finish_cancellable(mut self, cancellation: &Cancellation) -> Result<(), CliError> {
        self.publish(cancellation)
    }

    /// Header, PCM, flush, and file sync succeed before the same-directory rename.
    fn publish(&mut self, cancellation: &Cancellation) -> Result<(), CliError> {
        cancellation.check()?;
        if consume_wav_fault(WAV_FAULT_FLUSH) {
            return Err(CliError::WavFlushFailed);
        }
        self.writer.flush().map_err(|error| {
            CliError::io(CliIoOperation::FlushWav, self.temporary.clone(), error)
        })?;
        self.writer.get_ref().sync_all().map_err(|error| {
            CliError::io(CliIoOperation::SyncWav, self.temporary.clone(), error)
        })?;
        if consume_wav_fault(WAV_FAULT_RENAME) {
            return Err(CliError::WavRenameFailed);
        }
        // This is the commit boundary. A later cancellation must not delete or
        // relabel a successfully published file (nor hide a directory-sync error).
        cancellation.check()?;
        std::fs::rename(&self.temporary, &self.destination).map_err(|error| {
            CliError::io(CliIoOperation::RenameWav, self.destination.clone(), error)
        })?;
        self.published = true;
        if consume_wav_fault(WAV_FAULT_DIR_SYNC) {
            return Err(CliError::DurabilityUnknown {
                path: self.destination.clone(),
                error: None,
            });
        }
        sync_parent(&self.destination).map_err(|error| CliError::DurabilityUnknown {
            path: self.destination.clone(),
            error: Some(error),
        })
    }
}

fn wav_header(sample_rate: u32, frames: u32, format: WavFormat) -> Result<Vec<u8>, CliError> {
    let frame_bytes = u32::from(format.frame_bytes());
    let (riff_header_length, bits, subtype) = match format {
        WavFormat::Float32 => (58u64, 32u16, 3u16),
        WavFormat::Pcm16 => (44, 16, 1),
        WavFormat::Pcm24 => (68, 24, 1),
    };
    // The renderer's u32 frame count times a stereo frame always fits u64.
    let data_bytes = u64::from(frames) * u64::from(frame_bytes);
    let rf64 = data_bytes + riff_header_length - 8 > u64::from(u32::MAX);
    let float = matches!(format, WavFormat::Float32);
    let extensible = rf64 || matches!(format, WavFormat::Pcm24);
    let header_length = if rf64 {
        if float { 116 } else { 104 }
    } else {
        riff_header_length
    };
    let riff_size = data_bytes + header_length - 8;
    let byte_rate = sample_rate
        .checked_mul(frame_bytes)
        .ok_or(CliError::WavByteRate {
            sample_rate,
            frame_bytes,
        })?;
    let mut header = Vec::with_capacity(header_length as usize);
    header.extend_from_slice(if rf64 { b"RF64" } else { b"RIFF" });
    header.extend_from_slice(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    if rf64 {
        // EBU Tech 3306: ds64 is first and replaces both 0xffffffff size sentinels.
        header.extend_from_slice(b"ds64");
        header.extend_from_slice(&28u32.to_le_bytes());
        header.extend_from_slice(&riff_size.to_le_bytes());
        header.extend_from_slice(&data_bytes.to_le_bytes());
        header.extend_from_slice(&u64::from(frames).to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes()); // No additional large chunks.
    }
    header.extend_from_slice(b"fmt ");
    let format_length = if extensible {
        40u32
    } else if float {
        18
    } else {
        16
    };
    header.extend_from_slice(&format_length.to_le_bytes());
    header.extend_from_slice(&(if extensible { 0xfffeu16 } else { subtype }).to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&format.frame_bytes().to_le_bytes());
    header.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        // RF64 uses WAVEFORMATEXTENSIBLE for PCM and IEEE float alike.
        header.extend_from_slice(&22u16.to_le_bytes()); // cbSize.
        header.extend_from_slice(&bits.to_le_bytes()); // Valid bits.
        header.extend_from_slice(&3u32.to_le_bytes()); // Front left/right.
        header.extend_from_slice(&u32::from(subtype).to_le_bytes());
        header.extend_from_slice(&[0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
    } else if float {
        header.extend_from_slice(&0u16.to_le_bytes()); // WAVEFORMATEX cbSize.
    }
    if float {
        header.extend_from_slice(b"fact");
        header.extend_from_slice(&4u32.to_le_bytes());
        header.extend_from_slice(&(if rf64 { u32::MAX } else { frames }).to_le_bytes());
    }
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(if rf64 { u32::MAX } else { data_bytes as u32 }).to_le_bytes());
    debug_assert_eq!(header.len(), header_length as usize);
    Ok(header)
}

fn temporary_wav_path(destination: &Path) -> Result<PathBuf, CliError> {
    let name = destination.file_name().ok_or(CliError::WavFileName)?;
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

#[cfg(test)]
#[path = "wav_tests.rs"]
mod wav_tests;

#[cfg(test)]
#[path = "rf64_tests.rs"]
mod rf64_tests;

#[cfg(test)]
#[path = "dither_tests.rs"]
mod dither_tests;

#[cfg(test)]
mod tail_tests {
    #[test]
    fn every_clap_infinite_tail_value_requires_an_explicit_length() {
        for tail in [i32::MAX as u32, i32::MAX as u32 + 1, u32::MAX] {
            assert!(
                super::RenderTail::Automatic
                    .total_frames(1, tail)
                    .unwrap_err()
                    .to_string()
                    .contains("infinite plugin tail")
            );
        }
        assert_eq!(
            super::RenderTail::Automatic
                .total_frames(1, i32::MAX as u32 - 1)
                .unwrap(),
            i32::MAX as u32
        );
    }

    #[test]
    fn explicit_tails_ignore_infinite_device_reports_but_check_total_length() {
        use super::RenderTail;

        for device_tail in [0, i32::MAX as u32, u32::MAX] {
            for tail in [0, 1, i32::MAX as u32, u32::MAX - 1] {
                assert_eq!(
                    RenderTail::Frames(tail)
                        .total_frames(1, device_tail)
                        .unwrap(),
                    tail + 1
                );
            }
            assert_eq!(
                RenderTail::Frames(u32::MAX)
                    .total_frames(0, device_tail)
                    .unwrap(),
                u32::MAX
            );
            assert!(
                RenderTail::Frames(u32::MAX)
                    .total_frames(1, device_tail)
                    .unwrap_err()
                    .to_string()
                    .contains("export length overflowed")
            );
        }
        assert!(
            RenderTail::Automatic
                .total_frames(u32::MAX, 1)
                .unwrap_err()
                .to_string()
                .contains("export length overflowed")
        );
    }
}
