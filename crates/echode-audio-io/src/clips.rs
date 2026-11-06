//! Turn stored audio into device-rate clips on the control thread.
//!
//! The whole file is decoded and resampled here. Placements share unscaled
//! samples; gain, mono promotion and fades run when the block is mixed.
//! The callback only reads the slice that was stored on the plan. Offline
//! render uses the same clips.
//! The live loader retains one preparation across loop and transport changes,
//! so every loop head is resident before its plan can be adopted.
//!
//! Each source keeps at most one copy of its device-rate PCM. Short trims copy
//! only the frames they play and release the decoded file (M27-03). When the
//! distinct trims of a source add up to at least the source itself, every
//! placement reads a window of one shared copy instead (EXT-01).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use echode_engine::{
    ClipLoader, ClipWindow, LoadedClip, MediaInputs, PlaybackError, resample_into, tick_to_sample,
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
    /// Whole-source planes that windows of a shared source read.
    sources: HashMap<SourceKey, Planes>,
    windows: HashMap<WindowKey, Window>,
    #[cfg(test)]
    peak_sources: usize,
}

#[derive(Clone)]
pub(crate) struct Planes {
    pub(crate) left: Arc<[f32]>,
    pub(crate) right: Option<Arc<[f32]>>,
    // Cached once so gain edits can reject overflow without scanning the PCM.
    peak: f32,
}

impl Planes {
    fn new(left: Arc<[f32]>, right: Option<Arc<[f32]>>) -> Result<Self, PlaybackError> {
        let mut peak = 0.0f32;
        for &sample in left
            .iter()
            .chain(right.iter().flat_map(|plane| plane.iter()))
        {
            if !sample.is_finite() {
                return Err(PlaybackError::Media);
            }
            peak = peak.max(sample.abs());
        }
        Ok(Self { left, right, peak })
    }
}

/// The planes one placement shape reads.
#[derive(Clone)]
struct Window {
    /// The shared source, or a copy of just the frames played.
    planes: Planes,
    /// Where the placement reads a shared source. `None` plays all of `planes`.
    view: Option<ClipWindow>,
    /// The peak of the frames played, so a gain is judged by what it scales.
    peak: f32,
    shared: bool,
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

/// One audio placement with a nonzero window, in project order.
struct Placed<'a> {
    track: u128,
    audio: &'a AudioRef,
    window: WindowKey,
    start: i64,
    gain: f32,
    fade_in: u32,
    fade_out: u32,
}

fn placed_audio<'a>(project: &'a Project, rate: u32) -> Result<Vec<Placed<'a>>, PlaybackError> {
    let mut placed = Vec::new();
    for track in project.tracks() {
        for placement in &track.clips {
            let Some(audio) = project
                .content(placement.content)
                .and_then(|content| content.audio.as_ref())
            else {
                continue;
            };
            let gain = placement.gain;
            if !clip_gain_allowed(gain) {
                return Err(PlaybackError::Media);
            }
            if !matches!(audio.channels, 1 | 2) {
                return Err(PlaybackError::Unsupported(
                    "audio placement requires a mono or stereo layout".into(),
                ));
            }
            let start = tick_to_sample(placement.start, project.tempo(), rate)
                .ok_or(PlaybackError::Time)?;
            let end_tick = placement
                .start
                .get()
                .checked_add(placement.length.get())
                .ok_or(PlaybackError::Time)?;
            let end = tick_to_sample(Tick::new(end_tick), project.tempo(), rate)
                .ok_or(PlaybackError::Time)?;
            let frames = end.checked_sub(start).ok_or(PlaybackError::Time)?;
            let frames = usize::try_from(frames).map_err(|_| PlaybackError::Time)?;
            if frames == 0 {
                continue;
            }
            // Content position 0 is anchored at start - offset. The skipped
            // media is that interval's duration, not the interval after start.
            let lead = placement_media_lead(
                placement.start.get(),
                placement.offset.get(),
                project.tempo(),
                rate,
            )
            .ok_or(PlaybackError::Time)?;
            placed.push(Placed {
                track: track.id.raw(),
                audio,
                window: WindowKey {
                    source: source_key(audio, rate),
                    lead,
                    frames,
                },
                start,
                gain,
                fade_in: fade_frames(placement.fade_in, rate).ok_or(PlaybackError::Time)?,
                fade_out: fade_frames(placement.fade_out, rate).ok_or(PlaybackError::Time)?,
            });
        }
    }
    Ok(placed)
}

/// Sources whose distinct windows would copy at least the source's own
/// length. Those keep one copy and every placement reads a window of it.
fn shared_sources(placed: &[Placed<'_>]) -> Result<HashSet<SourceKey>, PlaybackError> {
    let mut copied = HashMap::<&SourceKey, u128>::new();
    let mut seen = HashSet::<&WindowKey>::new();
    for placement in placed {
        if seen.insert(&placement.window) {
            *copied.entry(&placement.window.source).or_default() += placement.window.frames as u128;
        }
    }
    let mut shared = HashSet::new();
    for (source, frames) in copied {
        let file = usize::try_from(source.frames).map_err(|_| PlaybackError::Time)?;
        let own = device_frames(file, source.file_rate, source.rate)?;
        if frames >= own as u128 {
            shared.insert(source.clone());
        }
    }
    Ok(shared)
}

/// The largest magnitude among the frames `window` plays; silence outside
/// the plane adds nothing.
fn window_peak(planes: &Planes, window: ClipWindow) -> f32 {
    let len = planes.left.len();
    let from = usize::try_from(window.lead.max(0))
        .unwrap_or(usize::MAX)
        .min(len);
    let to = i64::try_from(window.frames)
        .ok()
        .and_then(|frames| window.lead.checked_add(frames))
        .map_or(len, |end| {
            usize::try_from(end.max(0)).unwrap_or(usize::MAX).min(len)
        })
        .max(from);
    std::iter::once(&planes.left)
        .chain(planes.right.as_ref())
        .flat_map(|plane| plane[from..to].iter())
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
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
    let placed = placed_audio(project, device_rate)?;
    let shared = shared_sources(&placed)?;
    let mut remaining = HashMap::<&SourceKey, usize>::new();
    for placement in &placed {
        *remaining.entry(&placement.window.source).or_default() += 1;
    }
    let mut loaded = Vec::with_capacity(placed.len());
    let mut sources = HashMap::<SourceKey, Planes>::new();
    let mut windows = HashMap::<WindowKey, Window>::new();
    #[cfg(test)]
    let mut peak_sources = 0;
    for placement in &placed {
        let key = &placement.window;
        let is_shared = shared.contains(&key.source);
        if let Some(kept) = previous.and_then(|p| p.sources.get(&key.source)) {
            sources
                .entry(key.source.clone())
                .or_insert_with(|| kept.clone());
        }
        let window = if let Some(window) = windows.get(key) {
            window.clone()
        } else {
            // A window of the other kind would hold a second copy of the source.
            let window = if let Some(kept) = previous
                .and_then(|p| p.windows.get(key))
                .filter(|kept| kept.shared == is_shared)
            {
                kept.clone()
            } else {
                if !sources.contains_key(&key.source) {
                    sources.insert(
                        key.source.clone(),
                        source_planes(package, placement.audio, device_rate)?,
                    );
                    #[cfg(test)]
                    {
                        peak_sources = peak_sources.max(sources.len());
                    }
                }
                let source = sources.get(&key.source).ok_or(PlaybackError::Media)?;
                if is_shared {
                    let view = ClipWindow {
                        lead: key.lead,
                        frames: key.frames,
                    };
                    Window {
                        peak: window_peak(source, view),
                        planes: source.clone(),
                        view: (key.lead != 0 || key.frames != source.left.len()).then_some(view),
                        shared: true,
                    }
                } else {
                    let planes = Planes::new(
                        placed_plane(&source.left, key.lead, key.frames)?,
                        source
                            .right
                            .as_ref()
                            .map(|plane| placed_plane(plane, key.lead, key.frames))
                            .transpose()?,
                    )?;
                    Window {
                        peak: planes.peak,
                        planes,
                        view: None,
                        shared: false,
                    }
                }
            };
            windows.insert(key.clone(), window.clone());
            window
        };
        if !(window.peak * placement.gain).is_finite() {
            return Err(PlaybackError::Media);
        }
        loaded.push(LoadedClip {
            track: placement.track,
            start_sample: placement.start,
            samples: window.planes.left,
            right: window.planes.right,
            window: window.view,
            gain: placement.gain,
            fade_in: placement.fade_in,
            fade_out: placement.fade_out,
        });
        let count = remaining.get_mut(&key.source).ok_or(PlaybackError::Media)?;
        *count = count.checked_sub(1).ok_or(PlaybackError::Media)?;
        // Release a copied source's whole-file planes at its last placement,
        // rather than accumulating them across unrelated short trims.
        if *count == 0 && !is_shared {
            sources.remove(&key.source);
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
    Planes::new(planes.pop().ok_or(PlaybackError::Media)?, right)
}

fn placed_plane(
    source: &Arc<[f32]>,
    lead: i64,
    window: usize,
) -> Result<Arc<[f32]>, PlaybackError> {
    if lead == 0 && source.len() == window {
        if source.iter().any(|sample| !sample.is_finite()) {
            return Err(PlaybackError::Media);
        }
        return Ok(Arc::clone(source));
    }
    let samples = (0..window)
        .map(|index| {
            let sample = placed_sample(source, lead, index)?;
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
