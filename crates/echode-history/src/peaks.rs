//! Min/max peaks for an imported file, cached beside the object store.
//!
//! The pyramid is built on the document thread from the canonical PCM, before
//! clip gain and fades. Those are applied when the block is mixed. The arrange
//! shader draws only the columns that cover the visible clip. A column uses the
//! same source frames playback reads: the placement offset and length, converted
//! at the file's sample rate. Samples past the file are silence. `cache/` is
//! already gitignored, and a changed builder version ignores an old file.

use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use echode_model::{Id, Project, media_sample_at};

use crate::document::PackageError;
use crate::media::read_pcm;
use crate::objects::is_content_hash;

const MAGIC: &[u8; 8] = b"ECHDPK01";
/// Samples in one level-0 column. Coarser levels pair those columns.
const BUCKET: u32 = 64;

/// One envelope column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakColumn {
    pub min: f32,
    pub max: f32,
}

/// Pyramid plus a way to read fewer than `bucket` samples.
///
/// A cache hit does not read the canonical PCM. Coarse columns use the
/// pyramid. A partial bucket reads that range from a read-only map of the
/// object, which stays sparse until those pages are touched.
struct Peaks {
    frames: u64,
    bucket: u32,
    levels: Vec<Vec<PeakColumn>>,
    /// Samples already decoded. Used when the object could not be mapped.
    mono: Option<Arc<[f32]>>,
    object: Option<MappedPcm>,
}

impl std::fmt::Debug for Peaks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peaks")
            .field("frames", &self.frames)
            .field("bucket", &self.bucket)
            .field("levels", &self.levels.len())
            .field("mapped", &self.object.is_some())
            .finish()
    }
}

struct MappedPcm {
    package: PathBuf,
    hash: String,
    channels: u16,
    mapped: OnceLock<Option<memmap2::Mmap>>,
}

/// Peaks for the audio hashes a project currently references.
#[derive(Clone, Debug, Default)]
pub struct Waveforms {
    entries: HashMap<String, Arc<Peaks>>,
}

impl Waveforms {
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Loads every referenced file. A hash still held by `previous` is reused.
    pub fn load(package: &Path, project: &Project, previous: Option<&Self>) -> Self {
        let mut entries = HashMap::new();
        for content in project.contents() {
            let Some(audio) = content.audio.as_ref() else {
                continue;
            };
            if entries.contains_key(audio.hash.as_str()) {
                continue;
            }
            if let Some(kept) = previous.and_then(|previous| previous.entries.get(&audio.hash)) {
                entries.insert(audio.hash.clone(), Arc::clone(kept));
                continue;
            }
            if let Some(peaks) = load_hash(package, &audio.hash, audio.channels, audio.frames) {
                entries.insert(audio.hash.clone(), Arc::new(peaks));
            }
        }
        Self { entries }
    }

    /// Writes `columns` covering the source frames playback would read.
    ///
    /// Returns how many columns were written. A view past the end of the file
    /// is silence, the same value playback uses when the offset runs out.
    pub fn fill(
        &self,
        project: &Project,
        placement: Id,
        start_tick: i64,
        end_tick: i64,
        columns: &mut [PeakColumn],
    ) -> usize {
        let Some((clip_start, clip_length, offset, rate, hash, frames)) =
            placement_audio(project, placement)
        else {
            return 0;
        };
        let Some(peaks) = self.entries.get(&hash) else {
            return 0;
        };
        if peaks.frames != frames {
            return 0;
        }
        let clip_end = match clip_start.checked_add(clip_length) {
            Some(end) => end,
            None => return 0,
        };
        let left = start_tick.max(clip_start);
        let right = end_tick.min(clip_end);
        if right <= left || columns.is_empty() {
            return 0;
        }
        let Some(origin) = clip_start.checked_sub(offset) else {
            return 0;
        };
        columns.fill(PeakColumn { min: 0.0, max: 0.0 });
        let sound_left = left.max(origin);
        if right <= sound_left {
            // The visible window is inside the clip and before the file.
            return columns.len();
        }
        let Some((first, last)) = source_span(
            project.tempo(),
            SourceSpan {
                start: clip_start,
                length: clip_length,
                offset,
                rate,
                frames,
            },
            sound_left,
            right,
        ) else {
            return columns.len();
        };
        if first >= frames {
            return columns.len();
        }
        let span = end_tick.saturating_sub(start_tick);
        if span <= 0 {
            return columns.len();
        }
        let from = column_at(sound_left, start_tick, span, columns.len());
        let mut to = column_at(right, start_tick, span, columns.len());
        if to <= from {
            to = (from + 1).min(columns.len());
        }
        let to = to.min(columns.len());
        if from >= to {
            return columns.len();
        }
        fill_columns(peaks, first, last.min(frames), &mut columns[from..to]);
        columns.len()
    }
}

/// Builds and stores the pyramid for a PCM object. Import ignores a failure
/// and the next snapshot builds it again.
pub fn cache_peaks(package: &Path, hash: &str) -> Result<(), PackageError> {
    let pcm = read_pcm(package, hash)?;
    let mono = fold_mono(&pcm.samples, pcm.channels);
    let peaks = build_peaks(&mono, pcm.frames);
    write_cache(package, hash, &peaks)
}

fn load_hash(package: &Path, hash: &str, channels: u16, frames: u64) -> Option<Peaks> {
    if let Some(cached) = read_cache(package, hash)
        && cached.frames == frames
        && cached.bucket == BUCKET
    {
        return Some(Peaks {
            frames: cached.frames,
            bucket: cached.bucket,
            levels: cached.levels,
            mono: None,
            object: MappedPcm::open(package, hash, channels),
        });
    }
    let pcm = read_pcm(package, hash).ok()?;
    if pcm.frames != frames || pcm.channels != channels {
        return None;
    }
    let mono = fold_mono(&pcm.samples, pcm.channels);
    let mut peaks = build_peaks(&mono, pcm.frames);
    let _ = write_cache(package, hash, &peaks);
    peaks.object = MappedPcm::open(package, hash, channels);
    if peaks.object.is_none() {
        peaks.mono = Some(Arc::from(mono));
    }
    Some(peaks)
}

fn placement_audio(project: &Project, placement: Id) -> Option<(i64, i64, i64, u32, String, u64)> {
    for track in project.tracks() {
        for clip in &track.clips {
            if clip.id != placement {
                continue;
            }
            let audio = project.content(clip.content)?.audio.as_ref()?;
            let length = clip.length.get();
            if length <= 0 {
                return None;
            }
            return Some((
                clip.start.get(),
                length,
                clip.offset.get(),
                audio.sample_rate,
                audio.hash.clone(),
                audio.frames,
            ));
        }
    }
    None
}

#[derive(Clone, Copy)]
struct SourceSpan {
    start: i64,
    length: i64,
    offset: i64,
    rate: u32,
    frames: u64,
}

/// File frames playback reads for the visible part of a placement.
fn source_span(
    tempo: &echode_model::TempoMap,
    source: SourceSpan,
    start_tick: i64,
    end_tick: i64,
) -> Option<(u64, u64)> {
    if source.length <= 0 || source.frames == 0 || end_tick <= start_tick || source.rate == 0 {
        return None;
    }
    let clip_end = source.start.checked_add(source.length)?;
    let left = start_tick.max(source.start);
    let right = end_tick.min(clip_end);
    if right <= left {
        return None;
    }
    let origin_tick = source.start.checked_sub(source.offset)?;
    let sound_left = left.max(origin_tick);
    if right <= sound_left {
        return None;
    }
    let first = media_sample_at(origin_tick, sound_left, tempo, source.rate)?;
    let last = media_sample_at(origin_tick, right, tempo, source.rate)?;
    if last <= first || first < 0 {
        return None;
    }
    let first = u64::try_from(first).ok()?;
    let last = u64::try_from(last).ok()?;
    (last > first).then_some((first, last))
}

fn column_at(tick: i64, start: i64, span: i64, columns: usize) -> usize {
    if columns == 0 || tick <= start || span <= 0 {
        return 0;
    }
    if tick >= start.saturating_add(span) {
        return columns;
    }
    let index = i128::from(tick - start) * i128::from(u64::try_from(columns).unwrap_or(u64::MAX))
        / i128::from(span);
    usize::try_from(index).unwrap_or(columns).min(columns)
}

fn fill_columns(peaks: &Peaks, start: u64, end: u64, columns: &mut [PeakColumn]) -> usize {
    if columns.is_empty() || end <= start {
        return 0;
    }
    let count = u64::try_from(columns.len()).unwrap_or(u64::MAX);
    let span = end - start;
    for (index, column) in columns.iter_mut().enumerate() {
        let index = u64::try_from(index).unwrap_or(count);
        let mut left = start + mul_div(span, index, count);
        let mut right = start + mul_div(span, index.saturating_add(1), count);
        if right <= left {
            right = left.saturating_add(1).min(end);
        }
        if right <= left {
            left = end.saturating_sub(1);
            right = end;
        }
        *column = reduce(peaks, left, right);
    }
    columns.len()
}

fn mul_div(value: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    u64::try_from(u128::from(value) * u128::from(numerator) / u128::from(denominator))
        .unwrap_or(value)
}

fn reduce(peaks: &Peaks, start: u64, end: u64) -> PeakColumn {
    let end = end.min(peaks.frames);
    let start = start.min(end);
    if end <= start {
        return PeakColumn { min: 0.0, max: 0.0 };
    }
    let bucket = u64::from(peaks.bucket).max(1);
    if peaks.levels.is_empty() {
        return peaks.read_samples(start, end);
    }
    // Whole pyramid cells stay inside the requested range. Only a remainder
    // shorter than one bucket reads the object, so a cache hit does not load
    // the file until the view is finer than 64 samples at an unaligned edge.
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut cursor = start;
    while cursor < end {
        let remain = end - cursor;
        if !cursor.is_multiple_of(bucket) || remain < bucket {
            let edge_end = if cursor.is_multiple_of(bucket) {
                end
            } else {
                (cursor + (bucket - cursor % bucket)).min(end)
            };
            let edge = peaks.read_samples(cursor, edge_end);
            min = min.min(edge.min);
            max = max.max(edge.max);
            cursor = edge_end;
            continue;
        }
        let mut size = bucket;
        let mut level = 0usize;
        while level + 1 < peaks.levels.len() {
            let next = size.saturating_mul(2);
            if next < size || !cursor.is_multiple_of(next) || remain < next {
                break;
            }
            size = next;
            level += 1;
        }
        let index = usize::try_from(cursor / size).unwrap_or(usize::MAX);
        let Some(column) = peaks
            .levels
            .get(level)
            .and_then(|columns| columns.get(index))
        else {
            let edge = peaks.read_samples(cursor, end);
            min = min.min(edge.min);
            max = max.max(edge.max);
            break;
        };
        min = min.min(column.min);
        max = max.max(column.max);
        cursor = cursor.saturating_add(size);
    }
    if min.is_finite() && max.is_finite() {
        PeakColumn { min, max }
    } else {
        PeakColumn { min: 0.0, max: 0.0 }
    }
}

fn scan(mono: &[f32], start: u64, end: u64) -> PeakColumn {
    let start = usize::try_from(start).unwrap_or(mono.len());
    let end = usize::try_from(end).unwrap_or(mono.len()).min(mono.len());
    if start >= end || start >= mono.len() {
        return PeakColumn { min: 0.0, max: 0.0 };
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for sample in &mono[start..end] {
        if !sample.is_finite() {
            continue;
        }
        min = min.min(*sample);
        max = max.max(*sample);
    }
    if min.is_finite() && max.is_finite() {
        PeakColumn { min, max }
    } else {
        PeakColumn { min: 0.0, max: 0.0 }
    }
}

fn build_peaks(mono: &[f32], frames: u64) -> Peaks {
    let bucket = usize::try_from(BUCKET).unwrap_or(64);
    let mut level = Vec::new();
    if bucket == 0 {
        return Peaks {
            frames,
            bucket: BUCKET,
            levels: Vec::new(),
            mono: Some(Arc::from(mono)),
            object: None,
        };
    }
    let usable = mono
        .len()
        .min(usize::try_from(frames).unwrap_or(mono.len()));
    for chunk in mono[..usable].chunks(bucket) {
        level.push(scan_slice(chunk));
    }
    if level.is_empty() && usable > 0 {
        level.push(scan_slice(&mono[..usable]));
    }
    let mut levels = Vec::new();
    if !level.is_empty() {
        levels.push(level);
        while levels.last().is_some_and(|level| level.len() > 1) {
            let previous = levels.last().map(Vec::as_slice).unwrap_or(&[]);
            let mut next = Vec::with_capacity(previous.len().div_ceil(2));
            for pair in previous.chunks(2) {
                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;
                for column in pair {
                    min = min.min(column.min);
                    max = max.max(column.max);
                }
                next.push(PeakColumn { min, max });
            }
            levels.push(next);
        }
    }
    Peaks {
        frames,
        bucket: BUCKET,
        levels,
        mono: None,
        object: None,
    }
}

impl Peaks {
    fn read_samples(&self, start: u64, end: u64) -> PeakColumn {
        if let Some(mono) = &self.mono {
            return scan(mono, start, end);
        }
        self.object
            .as_ref()
            .map(|object| object.scan(start, end))
            .unwrap_or(PeakColumn { min: 0.0, max: 0.0 })
    }
}

impl MappedPcm {
    fn open(package: &Path, hash: &str, channels: u16) -> Option<Self> {
        if !is_content_hash(hash) || !matches!(channels, 1 | 2) {
            return None;
        }
        Some(Self {
            package: package.to_path_buf(),
            hash: hash.to_string(),
            channels,
            mapped: OnceLock::new(),
        })
    }

    fn scan(&self, start: u64, end: u64) -> PeakColumn {
        let Some(map) = self.mapped.get_or_init(|| self.map_object()).as_ref() else {
            return PeakColumn { min: 0.0, max: 0.0 };
        };
        let width = usize::from(self.channels) * 4;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for frame in start..end {
            let offset = 24usize.saturating_add(
                usize::try_from(frame)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(width),
            );
            let Some(bytes) = map.get(offset..offset.saturating_add(width)) else {
                break;
            };
            let mut sum = 0.0f32;
            let mut finite = true;
            for channel in 0..usize::from(self.channels) {
                let sample = f32::from_le_bytes(
                    bytes[channel * 4..channel * 4 + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                if !sample.is_finite() {
                    finite = false;
                    break;
                }
                sum += sample;
            }
            if !finite {
                continue;
            }
            let sample = sum / f32::from(self.channels);
            min = min.min(sample);
            max = max.max(sample);
        }
        if min.is_finite() && max.is_finite() {
            PeakColumn { min, max }
        } else {
            PeakColumn { min: 0.0, max: 0.0 }
        }
    }

    fn map_object(&self) -> Option<memmap2::Mmap> {
        let file = crate::package_io::Directory::open(&self.package)
            .ok()?
            .optional_child("media")
            .ok()??
            .optional_child("objects")
            .ok()??
            .open_file(&self.hash, false, false)
            .ok()?;
        let mut header = [0u8; 24];
        std::os::unix::fs::FileExt::read_exact_at(&file, &mut header, 0).ok()?;
        if &header[..8] != crate::media::PCM_MAGIC {
            return None;
        }
        let channels = u16::from_le_bytes(header[12..14].try_into().ok()?);
        let frames = u64::from_le_bytes(header[16..24].try_into().ok()?);
        if channels != self.channels || frames == 0 {
            return None;
        }
        let payload = frames.checked_mul(u64::from(channels))?.checked_mul(4)?;
        let bytes = 24u64.checked_add(payload)?;
        if file.metadata().ok()?.len() < bytes {
            return None;
        }
        // SAFETY: the directory opener rejects links and non-files. The map is
        // read-only. Canonical objects are immutable; a later truncate can
        // fault, and the header length check is the guard we have at map time.
        let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        if u64::try_from(map.len()).ok()? < bytes {
            return None;
        }
        Some(map)
    }
}

fn scan_slice(samples: &[f32]) -> PeakColumn {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for sample in samples {
        if !sample.is_finite() {
            continue;
        }
        min = min.min(*sample);
        max = max.max(*sample);
    }
    if min.is_finite() && max.is_finite() {
        PeakColumn { min, max }
    } else {
        PeakColumn { min: 0.0, max: 0.0 }
    }
}

fn fold_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let width = usize::from(channels);
    interleaved
        .chunks(width)
        .map(|frame| frame.iter().sum::<f32>() / f32::from(channels))
        .collect()
}

fn cache_path(package: &Path, hash: &str) -> Option<std::path::PathBuf> {
    if !is_content_hash(hash) {
        return None;
    }
    Some(package.join("cache").join("peaks").join(hash))
}

fn write_cache(package: &Path, hash: &str, peaks: &Peaks) -> Result<(), PackageError> {
    let Some(path) = cache_path(package, hash) else {
        return Err(PackageError::new("peak cache name is not a hash"));
    };
    let owned = crate::package_io::Directory::create(package)
        .and_then(|root| root.child("cache"))
        .and_then(|cache| cache.child("peaks"))
        .map_err(|error| PackageError::new(error.to_string()))?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&peaks.bucket.to_le_bytes());
    bytes.extend_from_slice(&peaks.frames.to_le_bytes());
    let count = u32::try_from(peaks.levels.len()).unwrap_or(u32::MAX);
    bytes.extend_from_slice(&count.to_le_bytes());
    for level in &peaks.levels {
        let columns = u32::try_from(level.len()).unwrap_or(u32::MAX);
        bytes.extend_from_slice(&columns.to_le_bytes());
        for column in level.iter().take(columns as usize) {
            bytes.extend_from_slice(&column.min.to_le_bytes());
            bytes.extend_from_slice(&column.max.to_le_bytes());
        }
    }
    owned
        .write_atomic(hash, &bytes, false)
        .map_err(|error| PackageError::new(format!("{}: {error}", path.display())))?;
    Ok(())
}

struct CachedPeaks {
    frames: u64,
    bucket: u32,
    levels: Vec<Vec<PeakColumn>>,
}

fn read_cache(package: &Path, hash: &str) -> Option<CachedPeaks> {
    cache_path(package, hash)?;
    let owned = crate::package_io::Directory::open(package)
        .ok()?
        .optional_child("cache")
        .ok()??
        .optional_child("peaks")
        .ok()??;
    let bytes = owned.read(hash).ok()?;
    if bytes.len() < 8 + 4 + 8 + 4 || &bytes[..8] != MAGIC {
        return None;
    }
    let mut cursor = 8usize;
    let bucket = read_u32(&bytes, &mut cursor)?;
    let frames = read_u64(&bytes, &mut cursor)?;
    let levels_count = read_u32(&bytes, &mut cursor)? as usize;
    let mut levels = Vec::with_capacity(levels_count);
    for _ in 0..levels_count {
        let columns = read_u32(&bytes, &mut cursor)? as usize;
        let mut level = Vec::with_capacity(columns);
        for _ in 0..columns {
            let min = read_f32(&bytes, &mut cursor)?;
            let max = read_f32(&bytes, &mut cursor)?;
            level.push(PeakColumn { min, max });
        }
        levels.push(level);
    }
    if cursor != bytes.len() || bucket != BUCKET {
        return None;
    }
    Some(CachedPeaks {
        frames,
        bucket,
        levels,
    })
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
    let next = cursor.checked_add(4)?;
    let value = u32::from_le_bytes(bytes.get(*cursor..next)?.try_into().ok()?);
    *cursor = next;
    Some(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let next = cursor.checked_add(8)?;
    let value = u64::from_le_bytes(bytes.get(*cursor..next)?.try_into().ok()?);
    *cursor = next;
    Some(value)
}

fn read_f32(bytes: &[u8], cursor: &mut usize) -> Option<f32> {
    let next = cursor.checked_add(4)?;
    let value = f32::from_le_bytes(bytes.get(*cursor..next)?.try_into().ok()?);
    *cursor = next;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_pro01_peak_cache_directory_links_are_not_followed() {
        for folder in ["cache", "peaks"] {
            let root = std::env::temp_dir()
                .join(format!("echode-peak-links-{}-{folder}", std::process::id()));
            let package = root.join("project.echode");
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::create_dir_all(package.join("cache")).unwrap();
            fs::write(outside.join("sentinel"), b"external").unwrap();
            let linked = if folder == "cache" {
                fs::remove_dir(package.join("cache")).unwrap();
                package.join("cache")
            } else {
                package.join("cache/peaks")
            };
            std::os::unix::fs::symlink(&outside, linked).unwrap();
            let hash = crate::objects::hash_bytes(b"audio");
            let peaks = build_peaks(&[0.25, -0.5], 2);
            assert!(write_cache(&package, &hash, &peaks).is_err());
            assert!(read_cache(&package, &hash).is_none());
            assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"external");
            assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_constant_and_an_impulse_keep_their_columns() {
        let mut mono = vec![0.25f32; 128];
        mono[70] = -0.5;
        mono[71] = 0.8;
        let peaks = build_peaks(&mono, 128);
        assert_eq!(peaks.levels[0].len(), 2);
        assert_eq!(
            peaks.levels[0][0],
            PeakColumn {
                min: 0.25,
                max: 0.25
            }
        );
        assert_eq!(peaks.levels[0][1].min, -0.5);
        assert_eq!(peaks.levels[0][1].max, 0.8);
        assert_eq!(peaks.levels.last().unwrap().len(), 1);
        assert_eq!(peaks.levels.last().unwrap()[0].min, -0.5);
        assert_eq!(peaks.levels.last().unwrap()[0].max, 0.8);
        let mut columns = vec![PeakColumn { min: 0.0, max: 0.0 }; 2];
        assert_eq!(fill_columns(&peaks, 0, 128, &mut columns), 2);
        assert_eq!(
            columns[0],
            PeakColumn {
                min: 0.25,
                max: 0.25
            }
        );
        assert_eq!(columns[1].min, -0.5);
        assert_eq!(columns[1].max, 0.8);
    }

    #[test]
    fn stereo_peaks_follow_the_average_and_round_trip() {
        let directory = std::env::temp_dir().join(format!(
            "echode-peaks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let interleaved = [1.0f32, -1.0, 0.5, 0.5];
        let mono = fold_mono(&interleaved, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
        let peaks = build_peaks(&mono, 2);
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        write_cache(&directory, hash, &peaks).unwrap();
        let cached = read_cache(&directory, hash).unwrap();
        let _ = fs::remove_dir_all(directory);
        assert_eq!(cached.frames, 2);
        assert_eq!(cached.levels[0][0], PeakColumn { min: 0.0, max: 0.5 });
    }

    #[test]
    fn a_tick_window_maps_onto_frames() {
        use echode_model::{TICKS_PER_QUARTER, Tempo, TempoMap};
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap());
        let quarter = TICKS_PER_QUARTER;
        let span = SourceSpan {
            start: 0,
            length: quarter,
            offset: 0,
            rate: 48_000,
            frames: 48_000,
        };
        let (first, last) = source_span(&tempo, span, 0, quarter).unwrap();
        assert_eq!((first, last), (0, 24_000));
        let (first, last) = source_span(&tempo, span, quarter / 2, quarter).unwrap();
        assert_eq!((first, last), (12_000, 24_000));
        assert!(source_span(&tempo, span, quarter, quarter + 1).is_none());
    }

    #[test]
    fn audit_rea10_trimmed_waveform_uses_source_offset() {
        use echode_model::{AudioRef, Id, Project, TICKS_PER_QUARTER, Tick};
        let directory = std::env::temp_dir().join(format!(
            "echode-rea10-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        crate::write_package(&directory, &Project::new()).unwrap();
        let frames = 96_000u64;
        let mut samples = vec![0.25f32; frames as usize];
        for sample in samples.iter_mut().skip(48_000) {
            *sample = 0.75;
        }
        let bytes = crate::media::encode_pcm(48_000, 1, frames, &samples).unwrap();
        let hash = crate::write_object(&directory, &bytes).unwrap();
        let (project, track) = Project::new().add_track("audio");
        let clip = Id::from_raw(4);
        let project = project
            .place_audio(
                track,
                clip,
                Id::from_raw(5),
                Tick::new(0),
                AudioRef::try_new(hash, 48_000, 1, frames).unwrap(),
            )
            .unwrap();
        let second = Tick::new(TICKS_PER_QUARTER * 2);
        let project = project.resize_clip(track, clip, second, second).unwrap();
        let waves = Waveforms::load(&directory, &project, None);
        let mut columns = vec![PeakColumn { min: 0.0, max: 0.0 }; 8];
        let wrote = waves.fill(&project, clip, second.get(), second.get() * 2, &mut columns);
        let _ = fs::remove_dir_all(&directory);
        assert_eq!(wrote, 8);
        for column in columns {
            assert!((column.min - 0.75).abs() < 1.0e-4, "{column:?}");
            assert!((column.max - 0.75).abs() < 1.0e-4, "{column:?}");
        }
    }

    #[test]
    fn a_peak_cache_answers_coarse_columns_without_the_object() {
        let directory = std::env::temp_dir().join(format!(
            "echode-peak-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        crate::write_package(&directory, &Project::new()).unwrap();
        let frames = 256u64;
        let mut samples = vec![0.0f32; frames as usize];
        samples[1] = 1.0;
        samples[64] = 0.25;
        let bytes = crate::media::encode_pcm(48_000, 1, frames, &samples).unwrap();
        let hash = crate::write_object(&directory, &bytes).unwrap();
        cache_peaks(&directory, &hash).unwrap();
        let exact = load_hash(&directory, &hash, 1, frames).unwrap();
        assert_eq!(reduce(&exact, 0, 1).max, 0.0);
        assert!((reduce(&exact, 1, 2).max - 1.0).abs() < 1.0e-6);
        // Sample 1 is outside [2, 65). An unaligned prefix must not take the
        // pyramid cell that starts at 0, and the sample at frame 64 is inside.
        assert!((reduce(&exact, 2, 65).max - 0.25).abs() < 1.0e-6);
        let cached = load_hash(&directory, &hash, 1, frames).unwrap();
        fs::remove_file(directory.join("media").join("objects").join(&hash)).unwrap();
        assert!((reduce(&cached, 0, 128).max - 1.0).abs() < 1.0e-6);
        assert!((reduce(&cached, 64, 128).max - 0.25).abs() < 1.0e-6);
        assert_eq!(reduce(&cached, 1, 2).max, 0.0);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn negative_offset_draws_silence_before_the_file() {
        use echode_model::{AudioRef, Id, Project, TICKS_PER_QUARTER, Tick};
        let directory = std::env::temp_dir().join(format!(
            "echode-neg-peaks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        crate::write_package(&directory, &Project::new()).unwrap();
        let frames = 48_000u64;
        let samples = vec![0.5f32; frames as usize];
        let bytes = crate::media::encode_pcm(48_000, 1, frames, &samples).unwrap();
        let hash = crate::write_object(&directory, &bytes).unwrap();
        let (project, track) = Project::new().add_track("audio");
        let clip = Id::from_raw(4);
        let two = Tick::new(TICKS_PER_QUARTER * 2);
        let project = project
            .place_audio(
                track,
                clip,
                Id::from_raw(5),
                two,
                AudioRef::try_new(hash, 48_000, 1, frames).unwrap(),
            )
            .unwrap()
            .resize_clip(track, clip, Tick::new(0), Tick::new(TICKS_PER_QUARTER * 4))
            .unwrap();
        let waves = Waveforms::load(&directory, &project, None);
        let mut columns = vec![PeakColumn { min: 1.0, max: 1.0 }; 8];
        let wrote = waves.fill(&project, clip, 0, TICKS_PER_QUARTER * 4, &mut columns);
        let _ = fs::remove_dir_all(&directory);
        assert_eq!(wrote, 8);
        for column in &columns[..4] {
            assert_eq!(column.min, 0.0, "{column:?}");
            assert_eq!(column.max, 0.0, "{column:?}");
        }
        for column in &columns[4..] {
            assert!((column.max - 0.5).abs() < 1.0e-3, "{column:?}");
        }
    }

    #[test]
    #[ignore = "timing measurement; run in a release build"]
    fn a_minute_of_peaks_is_timed() {
        let frames = 48_000 * 60;
        let mono: Vec<f32> = (0..frames)
            .map(|index| ((index % 64) as f32 / 32.0) - 1.0)
            .collect();
        let started = std::time::Instant::now();
        let peaks = build_peaks(&mono, frames as u64);
        let elapsed = started.elapsed();
        assert_eq!(peaks.levels[0].len(), frames / 64);
        eprintln!("cpu peaks for 60s mono: {elapsed:?}");
    }
}
