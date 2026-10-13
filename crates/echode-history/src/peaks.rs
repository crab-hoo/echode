//! Min/max peaks for an imported file, cached beside the object store.
//!
//! The pyramid is built on the document thread from the canonical PCM. The
//! arrange shader draws only the columns that cover the visible clip. `cache/`
//! is already gitignored, and a changed builder version ignores an old file.

use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::path::Path;
use std::sync::Arc;

use echode_model::{Id, Project};

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

#[derive(Clone, Debug)]
struct Peaks {
    frames: u64,
    bucket: u32,
    mono: Arc<[f32]>,
    levels: Vec<Vec<PeakColumn>>,
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
            if let Some(peaks) = load_hash(package, &audio.hash, audio.frames) {
                entries.insert(audio.hash.clone(), Arc::new(peaks));
            }
        }
        Self { entries }
    }

    /// Writes `columns` covering the audio frames between two ticks.
    ///
    /// The clip's tick span maps linearly onto its frame count, which is how
    /// the arrange rectangle is drawn. Returns how many columns were written.
    pub fn fill(
        &self,
        project: &Project,
        placement: Id,
        start_tick: i64,
        end_tick: i64,
        columns: &mut [PeakColumn],
    ) -> usize {
        let Some((clip_start, clip_length, hash, frames)) = placement_audio(project, placement)
        else {
            return 0;
        };
        let Some(peaks) = self.entries.get(&hash) else {
            return 0;
        };
        if peaks.frames != frames {
            return 0;
        }
        let Some((first, last)) = frame_span(clip_start, clip_length, frames, start_tick, end_tick)
        else {
            return 0;
        };
        fill_columns(peaks, first, last, columns)
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

fn load_hash(package: &Path, hash: &str, frames: u64) -> Option<Peaks> {
    let pcm = read_pcm(package, hash).ok()?;
    if pcm.frames != frames {
        return None;
    }
    let mono = fold_mono(&pcm.samples, pcm.channels);
    if let Some(cached) = read_cache(package, hash)
        && cached.frames == pcm.frames
        && cached.bucket == BUCKET
    {
        return Some(Peaks {
            frames: cached.frames,
            bucket: cached.bucket,
            mono: Arc::from(mono),
            levels: cached.levels,
        });
    }
    let peaks = build_peaks(&mono, pcm.frames);
    let _ = write_cache(package, hash, &peaks);
    Some(peaks)
}

fn placement_audio(project: &Project, placement: Id) -> Option<(i64, i64, String, u64)> {
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
            return Some((clip.start.get(), length, audio.hash.clone(), audio.frames));
        }
    }
    None
}

/// Maps a tick window onto frames of a clip that starts at `clip_start`.
fn frame_span(
    clip_start: i64,
    clip_length: i64,
    frames: u64,
    start_tick: i64,
    end_tick: i64,
) -> Option<(u64, u64)> {
    if clip_length <= 0 || frames == 0 || end_tick <= start_tick {
        return None;
    }
    let clip_end = clip_start.checked_add(clip_length)?;
    let left = start_tick.max(clip_start);
    let right = end_tick.min(clip_end);
    if right <= left {
        return None;
    }
    let map = |tick: i64| -> u64 {
        if tick <= clip_start {
            0
        } else if tick >= clip_end {
            frames
        } else {
            let delta = u128::from((tick - clip_start) as u64);
            u64::try_from(delta * u128::from(frames) / u128::from(clip_length as u64))
                .unwrap_or(frames)
        }
    };
    let first = map(left);
    let mut last = map(right);
    if last <= first {
        last = first.saturating_add(1).min(frames);
    }
    (last > first).then_some((first, last))
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
    let span = end - start;
    if span < u64::from(peaks.bucket) {
        return scan(&peaks.mono, start, end);
    }
    let mut level = 0usize;
    let mut size = u64::from(peaks.bucket);
    while level + 1 < peaks.levels.len() && size.saturating_mul(2) <= span {
        level += 1;
        size = size.saturating_mul(2);
    }
    let columns = &peaks.levels[level];
    if columns.is_empty() || size == 0 {
        return scan(&peaks.mono, start, end);
    }
    let first = usize::try_from(start / size).unwrap_or(0);
    let last = usize::try_from((end - 1) / size).unwrap_or(0);
    let last = last.min(columns.len() - 1);
    let first = first.min(last);
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for column in &columns[first..=last] {
        min = min.min(column.min);
        max = max.max(column.max);
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
            mono: Arc::from(mono),
            levels: Vec::new(),
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
        mono: Arc::from(mono),
        levels,
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
        let (first, last) = frame_span(0, 100, 50, 0, 100).unwrap();
        assert_eq!((first, last), (0, 50));
        let (first, last) = frame_span(0, 100, 50, 50, 100).unwrap();
        assert_eq!((first, last), (25, 50));
        assert!(frame_span(0, 100, 50, 100, 120).is_none());
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
