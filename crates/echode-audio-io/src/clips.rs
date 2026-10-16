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
            if !matches!(pcm.channels, 1 | 2) {
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
            let mut planes = Vec::with_capacity(usize::from(pcm.channels));
            for channel in 0..usize::from(pcm.channels) {
                let source: Vec<_> = pcm
                    .samples
                    .chunks_exact(usize::from(pcm.channels))
                    .map(|frame| frame[channel])
                    .collect();
                let mut device =
                    vec![0.0; device_frames(source.len(), pcm.sample_rate, device_rate)?];
                resample_into(&source, pcm.sample_rate, &mut device, device_rate)?;
                let samples: Arc<[f32]> = (0..window)
                    .map(|index| device.get(offset + index).copied().unwrap_or(0.0) * gain)
                    .collect();
                planes.push(samples);
            }
            let right = if pcm.channels == 2 {
                planes.pop()
            } else {
                None
            };
            loaded.push(LoadedClip {
                track: track.id.raw(),
                start_sample: start,
                samples: planes.pop().ok_or(PlaybackError::Media)?,
                right,
                fade_in: fade_frames(placement.fade_in, device_rate).ok_or(PlaybackError::Time)?,
                fade_out: fade_frames(placement.fade_out, device_rate)
                    .ok_or(PlaybackError::Time)?,
            });
        }
    }
    Ok(loaded)
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
mod tests {
    use super::*;
    use echode_engine::{PlaybackCommand, playback_pair, render_offline_blocks_with_clips};
    use echode_model::{AudioRef, Id};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn stereo_wav(rate: u32, frames: u32) -> Vec<u8> {
        let size = frames * 4;
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + size).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(size.to_le_bytes());
        for _ in 0..frames {
            wav.extend(16_384i16.to_le_bytes());
            wav.extend((-16_384i16).to_le_bytes());
        }
        wav
    }

    #[test]
    fn contract_imported_stereo_survives_trim_gain_resampling_playback_and_export() {
        let fixture =
            Fixture(std::env::temp_dir().join(format!("echode-stereo-{}", Id::generate().raw())));
        std::fs::create_dir_all(&fixture.0).unwrap();
        for file_rate in [44_100, 48_000] {
            let source = fixture.0.join(format!("opposite-{file_rate}.wav"));
            std::fs::write(&source, stereo_wav(file_rate, 4800)).unwrap();
            let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
            let (project, track) = Project::new().add_track("stereo");
            let clip = Id::generate();
            let project = project
                .place_audio(
                    track,
                    clip,
                    Id::generate(),
                    Tick::new(0),
                    AudioRef::try_new(
                        stored.hash,
                        stored.sample_rate,
                        stored.channels,
                        stored.frames,
                    )
                    .unwrap(),
                )
                .unwrap();
            let length = project.tracks()[0].clips[0].length.get();
            let project = project
                .resize_clip(track, clip, Tick::new(length / 4), Tick::new(length / 2))
                .unwrap()
                .set_clip_gain(clip, 0.5)
                .unwrap()
                .set_clip_fades(clip, 0, 0)
                .unwrap();
            let clips = load_placed_clips(&fixture.0, &project, 48_000).unwrap();
            assert_eq!(clips.len(), 1);
            let loaded = &clips[0];
            let right = loaded
                .right
                .as_ref()
                .expect("a stereo placement lost its right plane");
            assert!(loaded.start_sample > 0);
            assert_eq!(loaded.samples.len(), right.len());
            let mid = loaded.samples.len() / 2;
            assert!((loaded.samples[mid] - 0.25).abs() < 1.0e-4);
            assert!((right[mid] + 0.25).abs() < 1.0e-4);
            let frames = loaded.start_sample as usize + loaded.samples.len();
            // Scratch sizes exercise stream partitioning, not product limits.
            for block in [1, 64, 511] {
                let (mut control, mut audio) = playback_pair(&project, 48_000, block).unwrap();
                let clips = clips.clone();
                let expected = clips.clone();
                control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
                control.replace(&project).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let mut heard = Vec::new();
                for offset in (0..frames).step_by(block) {
                    let count = block.min(frames - offset);
                    let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
                    audio.render(&mut left, &mut right);
                    heard.extend(left.into_iter().zip(right).flat_map(|(l, r)| [l, r]));
                }
                assert!(!control.status().failed);
                let mut exported = Vec::new();
                render_offline_blocks_with_clips(
                    &project,
                    48_000,
                    frames as u32,
                    block as u32,
                    &expected,
                    |samples| {
                        exported.extend_from_slice(samples);
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
                assert_eq!(heard, exported);
                let at = (loaded.start_sample as usize + mid) * 2;
                assert!((heard[at] - 0.25).abs() < 1.0e-4);
                assert!((heard[at + 1] + 0.25).abs() < 1.0e-4);
            }
        }
    }
}
