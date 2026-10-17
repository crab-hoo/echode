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

use echode_engine::{ClipLoader, LoadedClip, PlaybackError, resample_into, tick_to_sample};
use echode_history::read_pcm;
use echode_model::{
    AudioRef, Id, Placement, Project, TempoMap, Tick, clip_gain_allowed, fade_frames,
    placement_media_lead,
};

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
    sources: HashMap<SourceKey, Planes>,
    windows: HashMap<WindowKey, Planes>,
    #[cfg(test)]
    peak_sources: usize,
}

#[derive(Clone)]
struct Planes {
    left: Arc<[f32]>,
    right: Option<Arc<[f32]>>,
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
    fn full_source_sharing_still_rejects_nonfinite_placement_samples() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("echode-finite-pcm-{}", Id::generate().raw())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        // This fixture exercises the whole-source fast path and a short trim;
        // its length is not a playback capacity or block-size limit.
        let frames = 4096u64;
        let mut bytes = b"ECHDPCM1".to_vec();
        bytes.extend(48000u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes.extend(frames.to_le_bytes());
        for index in 0..frames {
            bytes.extend(
                if index + 1 == frames {
                    f32::NAN
                } else {
                    0.25f32
                }
                .to_le_bytes(),
            );
        }
        let hash = echode_history::write_object(&fixture.0, &bytes).unwrap();
        let (project, track) = Project::new().add_track("finite boundary");
        let clip = Id::generate();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                Tick::new(0),
                AudioRef::try_new(hash, 48000, 1, frames).unwrap(),
            )
            .unwrap();
        assert!(
            load_placed_clips(&fixture.0, &project, 48000).is_err(),
            "whole-source sharing admitted NaN"
        );
        let trimmed = project
            .resize_clip(track, clip, Tick::new(0), Tick::new(7056))
            .unwrap();
        let loaded = load_placed_clips(&fixture.0, &trimmed, 48000).unwrap();
        assert!(loaded[0].samples.iter().all(|sample| *sample == 0.25));
    }

    #[test]
    fn short_trim_cache_does_not_pin_the_unused_whole_file() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("echode-trim-cache-{}", Id::generate().raw())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let source = fixture.0.join("long.wav");
        std::fs::write(&source, stereo_wav(48000, 48000)).unwrap();
        let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
        let (project, track) = Project::new().add_track("trim");
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
        let project = project
            .resize_clip(track, clip, Tick::new(0), Tick::new(7056))
            .unwrap();
        let second = Id::generate();
        let mut different = stereo_wav(48000, 48000);
        different[44..46].copy_from_slice(&8192i16.to_le_bytes());
        std::fs::write(&source, different).unwrap();
        let other = echode_history::import_audio(&fixture.0, &source).unwrap();
        let project = project
            .place_audio(
                track,
                second,
                Id::generate(),
                Tick::quarters(1).unwrap(),
                AudioRef::try_new(other.hash, other.sample_rate, other.channels, other.frames)
                    .unwrap(),
            )
            .unwrap()
            .resize_clip(track, second, Tick::quarters(1).unwrap(), Tick::new(7056))
            .unwrap();
        let prepared = prepare_media(&fixture.0, &project, 48000, None).unwrap();
        assert_eq!(
            prepared.peak_sources, 1,
            "temporary whole-file sources accumulated"
        );
        assert!(
            prepared.sources.is_empty(),
            "a short placement pinned the full source"
        );
        assert!(prepared.clips[0].samples.len() < 48000);
        std::fs::remove_dir_all(fixture.0.join("media/objects")).unwrap();
        let faded = project.set_clip_fades(clip, 7_056_000, 14_112_000).unwrap();
        let reused = prepare_media(&fixture.0, &faded, 48000, Some(&prepared)).unwrap();
        assert!(Arc::ptr_eq(
            &prepared.clips[0].samples,
            &reused.clips[0].samples
        ));
    }

    #[test]
    fn shared_pcm_survives_placement_edits_without_rereading_media() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("echode-shared-pcm-{}", Id::generate().raw())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        let source = fixture.0.join("shared.wav");
        std::fs::write(&source, stereo_wav(48000, 4096)).unwrap();
        let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
        let media = AudioRef::try_new(
            stored.hash,
            stored.sample_rate,
            stored.channels,
            stored.frames,
        )
        .unwrap();
        let (mut project, track) = Project::new().add_track("shared");
        let first = Id::generate();
        for (index, id) in [first, Id::generate()].into_iter().enumerate() {
            project = project
                .place_audio(
                    track,
                    id,
                    Id::generate(),
                    Tick::quarters(index as i64).unwrap(),
                    media.clone(),
                )
                .unwrap();
        }
        let clips = load_placed_clips(&fixture.0, &project, 48000).unwrap();
        assert!(
            Arc::ptr_eq(&clips[0].samples, &clips[1].samples),
            "same PCM was copied per placement"
        );
        assert!(Arc::ptr_eq(
            clips[0].right.as_ref().unwrap(),
            clips[1].right.as_ref().unwrap()
        ));
        let loader = media_loader(fixture.0.clone());
        let before = loader(&project, 48000).unwrap();
        std::fs::remove_dir_all(fixture.0.join("media/objects")).unwrap();
        let changed = loader(&project.set_clip_gain(first, 0.25).unwrap(), 48000).unwrap();
        assert_eq!(changed[0].samples[0], 0.125);
        assert_eq!(changed[0].right.as_ref().unwrap()[0], -0.125);
        assert!(Arc::ptr_eq(&before[1].samples, &changed[1].samples));
        let restored = loader(&project, 48000).unwrap();
        assert!(Arc::ptr_eq(&before[0].samples, &restored[0].samples));
        assert!(
            loader(&project, 96000).is_err(),
            "a different rate reused the old source"
        );
        assert!(loader(&Project::new(), 48000).unwrap().is_empty());
        assert!(
            loader(&project, 48000).is_err(),
            "unreferenced media was not evicted"
        );
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
                let faded = loader(
                    &project.set_clip_fades(clip, 7_056_000, 14_112_000).unwrap(),
                    rate,
                )
                .unwrap();
                assert!(Arc::ptr_eq(&faded[0].samples, &expected[0].samples));
                assert_ne!(faded[0].fade_out, expected[0].fade_out);
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

    fn mono_wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let frames = samples.len() as u32;
        let size = frames * 2;
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + size).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(size.to_le_bytes());
        for sample in samples {
            wav.extend(sample.to_le_bytes());
        }
        wav
    }

    #[test]
    fn trimmed_audio_keeps_the_media_that_was_playing_at_the_new_start() {
        let rate = 48_000u32;
        let mut pcm = vec![8_192i16; 160_000];
        pcm[48_000] = 24_576;
        pcm[96_000] = 6_553;
        let fixture =
            Fixture(std::env::temp_dir().join(format!("echode-origin-{}", Id::generate().raw())));
        std::fs::create_dir_all(&fixture.0).unwrap();
        let source = fixture.0.join("mark.wav");
        std::fs::write(&source, mono_wav(rate, &pcm)).unwrap();
        let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
        let (project, track) = Project::new().add_track("trim");
        let clip = Id::generate();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                echode_model::Tick::new(0),
                AudioRef::try_new(
                    stored.hash,
                    stored.sample_rate,
                    stored.channels,
                    stored.frames,
                )
                .unwrap(),
            )
            .unwrap();
        let two = echode_model::Tick::quarters(2).unwrap();
        let project = project
            .resize_clip(track, clip, two, two)
            .unwrap()
            .set_clip_fades(clip, 0, 0)
            .unwrap()
            .with_tempo(two, echode_model::Tempo::quarters_per_minute(60).unwrap());
        let loaded = load_placed_clips(&fixture.0, &project, rate).unwrap();
        assert_eq!(loaded.len(), 1);
        // 24576 / 32768 = 0.75. The following interval at 60 BPM is not the
        // media that was sounding when the left edge moved.
        assert!(
            (loaded[0].samples[0] - 0.75).abs() < 1.0e-3,
            "lead sample {}",
            loaded[0].samples[0]
        );
        assert!((loaded[0].samples[0] - (6_553.0 / 32_768.0)).abs() > 0.2);
    }

    #[test]
    fn a_negative_offset_is_silence_until_the_media_origin() {
        let rate = 48_000u32;
        let pcm = vec![16_384i16; 48_000];
        let fixture =
            Fixture(std::env::temp_dir().join(format!("echode-negative-{}", Id::generate().raw())));
        std::fs::create_dir_all(&fixture.0).unwrap();
        let source = fixture.0.join("tone.wav");
        std::fs::write(&source, mono_wav(rate, &pcm)).unwrap();
        let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
        let (project, track) = Project::new().add_track("extend");
        let clip = Id::generate();
        let two = echode_model::Tick::quarters(2).unwrap();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                two,
                AudioRef::try_new(
                    stored.hash,
                    stored.sample_rate,
                    stored.channels,
                    stored.frames,
                )
                .unwrap(),
            )
            .unwrap()
            .resize_clip(
                track,
                clip,
                echode_model::Tick::new(0),
                echode_model::Tick::quarters(4).unwrap(),
            )
            .unwrap()
            .set_clip_fades(clip, 0, 0)
            .unwrap();
        assert!(project.tracks()[0].clips[0].offset.get() < 0);
        let loaded = load_placed_clips(&fixture.0, &project, rate).unwrap();
        assert_eq!(loaded[0].samples[0], 0.0);
        assert!((loaded[0].samples[48_000] - 0.5).abs() < 1.0e-3);
    }
}
