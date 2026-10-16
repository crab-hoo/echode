//! Turn stored audio into device-rate clips on the control thread.
//!
//! The whole file is decoded and resampled here, then the placement's linear
//! gain is baked into those samples. Fades still run when the block is mixed.
//! The callback only reads the slice that was stored on the plan. Offline
//! render uses the same clips.
//! The live loader retains one preparation across loop and transport changes,
//! so every loop head is resident before its plan can be adopted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use echode_engine::{ClipLoader, LoadedClip, PlaybackError, resample_into, tick_to_sample};
use echode_history::read_pcm;
use echode_model::{AudioRef, Id, Placement, Project, TempoMap, Tick, fade_frames};

#[derive(PartialEq, Eq)]
struct MediaKey {
    rate: u32,
    tempo: TempoMap,
    placements: Vec<(Id, Placement, AudioRef)>,
}

impl MediaKey {
    fn new(project: &Project, rate: u32) -> Self {
        let placements = project
            .tracks()
            .iter()
            .flat_map(|track| {
                track.clips.iter().filter_map(|clip| {
                    let audio = project.content(clip.content)?.audio.as_ref()?;
                    Some((track.id, clip.clone(), audio.clone()))
                })
            })
            .collect();
        Self {
            rate,
            tempo: project.tempo().clone(),
            placements,
        }
    }
}

struct PreparedMedia {
    key: MediaKey,
    clips: Vec<LoadedClip>,
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
        let clips = load_placed_clips(&package, project, rate)?;
        // Failed loading leaves the previous preparation available. The live
        // plan and this control-side owner share immutable sample planes.
        *prepared = Some(PreparedMedia {
            key,
            clips: clips.clone(),
        });
        Ok(clips)
    })
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
    fn contract_loop_head_is_resident_before_wrap_under_rtsan() {
        for rate in [48_000, 96_000] {
            for block in [1, 7, 64, 512] {
                let fixture = Fixture(
                    std::env::temp_dir().join(format!("echode-loop-head-{}", Id::generate().raw())),
                );
                std::fs::create_dir_all(&fixture.0).unwrap();
                let source = fixture.0.join("stereo.wav");
                std::fs::write(&source, stereo_wav(44_100, 4410)).unwrap();
                let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
                let (project, track) = Project::new().add_track("head");
                let clip = Id::generate();
                let project = project
                    .place_audio(
                        track,
                        clip,
                        Id::generate(),
                        Tick::quarters(1).unwrap(),
                        AudioRef::try_new(
                            stored.hash,
                            stored.sample_rate,
                            stored.channels,
                            stored.frames,
                        )
                        .unwrap(),
                    )
                    .unwrap()
                    .set_clip_gain(clip, 0.5)
                    .unwrap()
                    .set_clip_fades(clip, 0, 0)
                    .unwrap();
                let loader = media_loader(fixture.0.clone());
                let (mut control, mut audio) = playback_pair(&project, rate, block).unwrap();
                control.set_clip_loader(Arc::clone(&loader));
                control.replace(&project).unwrap();
                let expected = loader(&project, rate).unwrap();
                let start = expected[0].start_sample + 101;
                let end = start + 1009;
                // Only this temporary project's objects disappear. A prepared
                // loop must not re-read them or depend on a disk at its wrap.
                std::fs::remove_dir_all(fixture.0.join("media/objects")).unwrap();
                assert!(
                    control
                        .command(PlaybackCommand::SetLoop { start, end })
                        .is_ok(),
                    "loop head was discarded instead of retained before activation"
                );
                let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
                // Drain this fixture's queued plans while stopped.
                for _ in 0..4 {
                    audio.render(&mut left, &mut right);
                    control.service();
                }
                assert!(!control.status().pending_plan);
                control.command(PlaybackCommand::Locate(start)).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                // These scratch sizes cover three passes of this short loop.
                let total = 1009 * 3 + 37;
                let mut heard = Vec::new();
                for offset in (0..total).step_by(block) {
                    let n = block.min(total - offset);
                    audio.render(&mut left[..n], &mut right[..n]);
                    assert!(!control.status().failed);
                    heard.extend(left[..n].iter().copied().zip(right[..n].iter().copied()));
                    control.service();
                }
                let fade = echode_engine::seam_fade_frames(rate).min(1009 / 2);
                for (index, &(left, right)) in heard.iter().enumerate() {
                    let position = (index % 1009) as i64;
                    let gain = ((position as f32 / (fade - 1) as f32).min(1.0))
                        * (((1008 - position) as f32 / (fade - 1) as f32).min(1.0));
                    let source = (start + position - expected[0].start_sample) as usize;
                    assert_eq!(left, expected[0].samples[source] * gain);
                    assert_eq!(right, expected[0].right.as_ref().unwrap()[source] * gain);
                }
                assert!(
                    heard
                        .iter()
                        .any(|(left, right)| *left > 0.24 && *right < -0.24)
                );
                // Clearing/changing a loop also retains the prepared placement.
                assert!(control.command(PlaybackCommand::ClearLoop).is_ok());
                assert!(
                    control
                        .command(PlaybackCommand::SetLoop {
                            start: start + 7,
                            end
                        })
                        .is_ok()
                );
                // A new rate is a different preparation, never stale PCM.
                assert!(loader(&project, rate / 2).is_err());
                assert!(loader(&project.set_clip_gain(clip, 0.25).unwrap(), rate).is_err());
                assert!(loader(&project.set_clip_fades(clip, 1, 2).unwrap(), rate).is_err());
                let slower = project.with_tempo(
                    Tick::new(0),
                    echode_model::Tempo::quarters_per_minute(60).unwrap(),
                );
                assert!(loader(&slower, rate).is_err());
                let moved = project
                    .resize_clip(track, clip, Tick::new(0), Tick::new(100))
                    .unwrap();
                assert!(loader(&moved, rate).is_err());
                let gain = project.tracks()[0].devices[0].parameters[0].id;
                let mixed = project.set_parameter(gain, 0.75).unwrap();
                let retained = loader(&mixed, rate).unwrap();
                assert!(Arc::ptr_eq(&retained[0].samples, &expected[0].samples));
                assert!(
                    loader(&project, rate).is_ok(),
                    "failed preparation destroyed the retained head"
                );
                assert!(loader(&Project::new(), rate).unwrap().is_empty());
                assert!(
                    loader(&project, rate).is_err(),
                    "unused media was not evicted"
                );
            }
        }
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
