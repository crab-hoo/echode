//! Turn stored audio into device-rate clips on the control thread.
//!
//! The whole file is decoded and resampled here, then the placement's linear
//! gain is baked into those samples. Fades still run when the block is mixed.
//! The callback only reads the slice that was stored on the plan. Offline
//! render uses the same clips.
//! The live loader retains one preparation across loop and transport changes,
//! so every loop head is resident before its plan can be adopted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use echode_engine::{
    ClipLoader, LoadedClip, MediaInputs, PlaybackError, resample_into, tick_to_sample,
};
use echode_history::{DecodedPcm, read_pcm};
use echode_model::{AudioRef, Project, Tick, clip_gain_allowed, fade_frames, placement_media_lead};

#[derive(PartialEq, Eq)]
struct MediaKey {
    rate: u32,
    inputs: MediaInputs,
}

impl MediaKey {
    fn new(project: &Project, rate: u32) -> Self {
        Self {
            rate,
            inputs: MediaInputs::of(project),
        }
    }
}

struct PreparedMedia {
    key: MediaKey,
    clips: Vec<LoadedClip>,
    sources: HashMap<SourceKey, Planes>,
    windows: HashMap<WindowKey, Planes>,
    #[cfg(test)]
    peak_sources: usize,
}

#[derive(Clone)]
pub(crate) struct Planes {
    pub(crate) left: Arc<[f32]>,
    pub(crate) right: Option<Arc<[f32]>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SourceKey {
    hash: String,
    rate: u32,
    file_rate: u32,
    channels: u16,
    frames: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct WindowKey {
    source: SourceKey,
    lead: i64,
    frames: usize,
    gain: u32,
}

fn source_key(audio: &AudioRef, rate: u32) -> SourceKey {
    SourceKey {
        hash: audio.hash.clone(),
        rate,
        file_rate: audio.sample_rate,
        channels: audio.channels,
        frames: audio.frames,
    }
}

fn finish_source(
    key: &SourceKey,
    remaining: &mut HashMap<SourceKey, usize>,
    sources: &mut HashMap<SourceKey, Planes>,
    published: &HashSet<SourceKey>,
) -> Result<(), PlaybackError> {
    let count = remaining.get_mut(key).ok_or(PlaybackError::Media)?;
    *count = count.checked_sub(1).ok_or(PlaybackError::Media)?;
    // Release temporary whole-file storage at its last placement, rather than
    // accumulating it across unrelated short trims. Published planes stay owned.
    if *count == 0 && !published.contains(key) {
        sources.remove(key);
    }
    Ok(())
}

pub(crate) fn media_loader(package: PathBuf) -> ClipLoader {
    // Only PlaybackControl invokes this closure. Keep one successful preparation
    // resident; loop/transport/fader/note edits must not read the same PCM again.
    let prepared = Mutex::new(None::<PreparedMedia>);
    Arc::new(move |project, rate| {
        let mut prepared = prepared.lock().map_err(|_| PlaybackError::Media)?;
        let key = MediaKey::new(project, rate);
        if let Some(previous) = prepared.as_ref()
            && previous.key == key
        {
            return Ok(previous.clips.clone());
        }
        let next = prepare_media(&package, project, rate, prepared.as_ref())?;
        let clips = next.clips.clone();
        // Failed loading leaves the previous preparation available. The live
        // plan and this control-side owner share immutable sample planes.
        *prepared = Some(next);
        Ok(clips)
    })
}

pub fn load_placed_clips(
    package: &Path,
    project: &Project,
    device_rate: u32,
) -> Result<Vec<LoadedClip>, PlaybackError> {
    Ok(prepare_media(package, project, device_rate, None)?.clips)
}

fn prepare_media(
    package: &Path,
    project: &Project,
    device_rate: u32,
    previous: Option<&PreparedMedia>,
) -> Result<PreparedMedia, PlaybackError> {
    if device_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    let mut loaded = Vec::new();
    let mut sources = HashMap::<SourceKey, Planes>::new();
    let mut windows = HashMap::<WindowKey, Planes>::new();
    let mut remaining = HashMap::<SourceKey, usize>::new();
    let mut published = HashSet::new();
    for track in project.tracks() {
        for placement in &track.clips {
            if let Some(audio) = project
                .content(placement.content)
                .and_then(|content| content.audio.as_ref())
            {
                *remaining.entry(source_key(audio, device_rate)).or_default() += 1;
            }
        }
    }
    #[cfg(test)]
    let mut peak_sources = 0;
    for track in project.tracks() {
        for placement in &track.clips {
            let Some(content) = project.content(placement.content) else {
                continue;
            };
            let Some(audio) = content.audio.as_ref() else {
                continue;
            };
            let source_key = source_key(audio, device_rate);
            let gain = placement.gain;
            if !clip_gain_allowed(gain) {
                return Err(PlaybackError::Media);
            }
            if !matches!(audio.channels, 1 | 2) {
                return Err(PlaybackError::Unsupported(
                    "audio placement requires a mono or stereo layout".into(),
                ));
            }
            let start = tick_to_sample(placement.start, project.tempo(), device_rate)
                .ok_or(PlaybackError::Time)?;
            let end_tick = placement
                .start
                .get()
                .checked_add(placement.length.get())
                .ok_or(PlaybackError::Time)?;
            let end = tick_to_sample(Tick::new(end_tick), project.tempo(), device_rate)
                .ok_or(PlaybackError::Time)?;
            let window = end.checked_sub(start).ok_or(PlaybackError::Time)?;
            let window = usize::try_from(window).map_err(|_| PlaybackError::Time)?;
            if window == 0 {
                finish_source(&source_key, &mut remaining, &mut sources, &published)?;
                continue;
            }
            // Content position 0 is anchored at start - offset. The skipped
            // media is that interval's duration, not the interval after start.
            let lead = placement_media_lead(
                placement.start.get(),
                placement.offset.get(),
                project.tempo(),
                device_rate,
            )
            .ok_or(PlaybackError::Time)?;
            if let Some(kept) = previous.and_then(|p| p.sources.get(&source_key)) {
                sources
                    .entry(source_key.clone())
                    .or_insert_with(|| kept.clone());
            }
            let window_key = WindowKey {
                source: source_key.clone(),
                lead,
                frames: window,
                gain: gain.to_bits(),
            };
            let planes = if let Some(planes) = windows.get(&window_key) {
                planes.clone()
            } else {
                let planes = if let Some(kept) = previous.and_then(|p| p.windows.get(&window_key)) {
                    kept.clone()
                } else {
                    if !sources.contains_key(&source_key) {
                        sources.insert(
                            source_key.clone(),
                            source_planes(package, audio, device_rate)?,
                        );
                        #[cfg(test)]
                        {
                            peak_sources = peak_sources.max(sources.len());
                        }
                    }
                    let source = sources.get(&source_key).ok_or(PlaybackError::Media)?;
                    Planes {
                        left: placed_plane(&source.left, lead, window, gain)?,
                        right: source
                            .right
                            .as_ref()
                            .map(|plane| placed_plane(plane, lead, window, gain))
                            .transpose()?,
                    }
                };
                windows.insert(window_key, planes.clone());
                planes
            };
            if sources
                .get(&source_key)
                .is_some_and(|source| Arc::ptr_eq(&source.left, &planes.left))
            {
                published.insert(source_key.clone());
            }
            loaded.push(LoadedClip {
                track: track.id.raw(),
                start_sample: start,
                samples: planes.left,
                right: planes.right,
                fade_in: fade_frames(placement.fade_in, device_rate).ok_or(PlaybackError::Time)?,
                fade_out: fade_frames(placement.fade_out, device_rate)
                    .ok_or(PlaybackError::Time)?,
            });
            finish_source(&source_key, &mut remaining, &mut sources, &published)?;
        }
    }
    Ok(PreparedMedia {
        key: MediaKey::new(project, device_rate),
        clips: loaded,
        sources,
        windows,
        #[cfg(test)]
        peak_sources,
    })
}

fn source_planes(package: &Path, audio: &AudioRef, rate: u32) -> Result<Planes, PlaybackError> {
    let pcm = read_pcm(package, &audio.hash).map_err(media_error)?;
    if (pcm.sample_rate, pcm.channels, pcm.frames)
        != (audio.sample_rate, audio.channels, audio.frames)
    {
        return Err(PlaybackError::Media);
    }
    device_planes(&pcm, rate)
}

/// Splits interleaved PCM into planes at the device `rate`, the same way for
/// placed clips and for a sample audition.
pub(crate) fn device_planes(pcm: &DecodedPcm, rate: u32) -> Result<Planes, PlaybackError> {
    if !matches!(pcm.channels, 1 | 2) {
        return Err(PlaybackError::Media);
    }
    let mut planes = Vec::with_capacity(usize::from(pcm.channels));
    for channel in 0..usize::from(pcm.channels) {
        let source: Vec<_> = pcm
            .samples
            .chunks_exact(usize::from(pcm.channels))
            .map(|frame| frame[channel])
            .collect();
        let samples = if pcm.sample_rate == rate {
            source
        } else {
            let mut device = vec![0.0; device_frames(source.len(), pcm.sample_rate, rate)?];
            resample_into(&source, pcm.sample_rate, &mut device, rate)?;
            device
        };
        planes.push(Arc::<[f32]>::from(samples));
    }
    let right = if pcm.channels == 2 {
        planes.pop()
    } else {
        None
    };
    Ok(Planes {
        left: planes.pop().ok_or(PlaybackError::Media)?,
        right,
    })
}

fn placed_plane(
    source: &Arc<[f32]>,
    lead: i64,
    window: usize,
    gain: f32,
) -> Result<Arc<[f32]>, PlaybackError> {
    if lead == 0 && source.len() == window && gain == 1.0 {
        if source.iter().any(|sample| !sample.is_finite()) {
            return Err(PlaybackError::Media);
        }
        return Ok(Arc::clone(source));
    }
    let samples = (0..window)
        .map(|index| {
            let sample = placed_sample(source, lead, index)? * gain;
            if sample.is_finite() {
                Ok(sample)
            } else {
                Err(PlaybackError::Media)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Arc::from(samples))
}

fn placed_sample(device: &[f32], lead: i64, index: usize) -> Result<f32, PlaybackError> {
    let index = i64::try_from(index).map_err(|_| PlaybackError::Time)?;
    let at = lead.checked_add(index).ok_or(PlaybackError::Time)?;
    Ok(usize::try_from(at)
        .ok()
        .and_then(|at| device.get(at).copied())
        .unwrap_or(0.0))
}

fn device_frames(
    file_frames: usize,
    file_rate: u32,
    device_rate: u32,
) -> Result<usize, PlaybackError> {
    if file_rate == 0 || device_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    if file_rate == device_rate {
        return Ok(file_frames);
    }
    let rounded = i128::from(u64::try_from(file_frames).map_err(|_| PlaybackError::Time)?)
        * i128::from(device_rate)
        + i128::from(file_rate) / 2;
    let frames = rounded / i128::from(file_rate);
    usize::try_from(frames).map_err(|_| PlaybackError::Time)
}

fn media_error(_error: echode_history::PackageError) -> PlaybackError {
    PlaybackError::Media
}

#[cfg(test)]
#[path = "clips_tests.rs"]
mod tests;
