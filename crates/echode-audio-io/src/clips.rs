//! Turn stored audio into device-rate clips on the control thread.
//!
//! The whole file is decoded and resampled here, then the placement's linear
//! gain is baked into those samples. Fades still run when the block is mixed.
//! The callback only reads the slice that was stored on the plan. Offline
//! render uses the same clips.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use echode_engine::{ClipLoader, LoadedClip, PlaybackError, resample_into, tick_to_sample};
use echode_history::read_pcm;
use echode_model::{Project, Tick, fade_frames};

pub(crate) fn media_loader(package: PathBuf) -> ClipLoader {
    Arc::new(move |project, rate| load_placed_clips(&package, project, rate))
}

pub fn load_placed_clips(
    package: &Path,
    project: &Project,
    device_rate: u32,
) -> Result<Vec<LoadedClip>, PlaybackError> {
    if device_rate == 0 {
        return Err(PlaybackError::InvalidSampleRate);
    }
    let mut loaded = Vec::new();
    for track in project.tracks() {
        for placement in &track.clips {
            let Some(content) = project.content(placement.content) else {
                continue;
            };
            let Some(audio) = content.audio.as_ref() else {
                continue;
            };
            let pcm = read_pcm(package, &audio.hash).map_err(media_error)?;
            if pcm.sample_rate != audio.sample_rate
                || pcm.channels != audio.channels
                || pcm.frames != audio.frames
            {
                return Err(PlaybackError::Media);
            }
            let gain = placement.gain;
            if !gain.is_finite() || gain < 0.0 {
                return Err(PlaybackError::Media);
            }
            let mono = fold_mono(&pcm.samples, pcm.channels);
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
                continue;
            }
            let offset_tick = placement
                .start
                .get()
                .checked_add(placement.offset.get())
                .ok_or(PlaybackError::Time)?;
            let offset_at = tick_to_sample(Tick::new(offset_tick), project.tempo(), device_rate)
                .ok_or(PlaybackError::Time)?;
            let offset = usize::try_from(offset_at.checked_sub(start).ok_or(PlaybackError::Time)?)
                .map_err(|_| PlaybackError::Time)?;
            let mut device = vec![0.0; device_frames(mono.len(), pcm.sample_rate, device_rate)?];
            resample_into(&mono, pcm.sample_rate, &mut device, device_rate)?;
            let mut samples = vec![0.0; window];
            for (index, sample) in samples.iter_mut().enumerate() {
                *sample = device.get(offset + index).copied().unwrap_or(0.0) * gain;
            }
            loaded.push(LoadedClip {
                track: track.id.raw(),
                start_sample: start,
                samples: Arc::from(samples),
                fade_in: fade_frames(placement.fade_in, device_rate).ok_or(PlaybackError::Time)?,
                fade_out: fade_frames(placement.fade_out, device_rate)
                    .ok_or(PlaybackError::Time)?,
            });
        }
    }
    Ok(loaded)
}

fn fold_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(usize::from(channels))
        .map(|frame| frame.iter().sum::<f32>() / f32::from(channels))
        .collect()
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
