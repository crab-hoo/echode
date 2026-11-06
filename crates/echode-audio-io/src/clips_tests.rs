use super::*;
use echode_engine::{PlaybackCommand, PlaybackOptions, playback, render_offline_blocks_with_clips};
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
    let fixture =
        Fixture(std::env::temp_dir().join(format!("echode-finite-pcm-{}", Id::generate().raw())));
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
    assert!(
        load_placed_clips(&fixture.0, &trimmed, 48000).is_err(),
        "trimming must not admit a corrupt stored source"
    );
}

#[test]
fn placement_planes_check_nonfinite_results_in_shared_and_copied_paths() {
    // This buffer length only separates the whole-source and trimmed test paths.
    let source: Arc<[f32]> = Arc::from([0.25, 0.5, f32::NAN]);
    assert!(placed_plane(&source, 0, source.len()).is_err());
    let trimmed = placed_plane(&source, 0, 2).unwrap();
    assert_eq!(&*trimmed, &[0.25, 0.5]);
    assert!(Planes::new(Arc::clone(&source), None).is_err());
    assert!(Planes::new(Arc::from([0.25]), Some(source)).is_err());
}

#[test]
fn cached_window_peaks_reject_gain_overflow_without_rereading_pcm() {
    for channels in [1u16, 2] {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("echode-gain-peak-{}", Id::generate().raw())),
        );
        std::fs::create_dir_all(&fixture.0).unwrap();
        // This source and its short trim only bound the finite-gain regression.
        let frames = 4096u64;
        let mut bytes = b"ECHDPCM1".to_vec();
        bytes.extend(48000u32.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes.extend(frames.to_le_bytes());
        for frame in 0..frames {
            for channel in 0..channels {
                let sample = if frame + 1 == frames && channel + 1 == channels {
                    -f32::MAX
                } else {
                    0.25f32
                };
                bytes.extend(sample.to_le_bytes());
            }
        }
        let hash = echode_history::write_object(&fixture.0, &bytes).unwrap();
        let (project, track) = Project::new().add_track("finite gain");
        let clip = Id::generate();
        let project = project
            .place_audio(
                track,
                clip,
                Id::generate(),
                Tick::new(0),
                AudioRef::try_new(hash, 48000, channels, frames).unwrap(),
            )
            .unwrap();
        let loader = media_loader(fixture.0.clone());
        let before = loader(&project, 48000).unwrap();
        std::fs::remove_dir_all(fixture.0.join("media/objects")).unwrap();
        assert!(matches!(
            loader(&project.set_clip_gain(clip, 2.0).unwrap(), 48000),
            Err(PlaybackError::Media)
        ));
        for gain in [0.0, 0.5, 1.0] {
            let changed = loader(&project.set_clip_gain(clip, gain).unwrap(), 48000).unwrap();
            assert_eq!(changed[0].gain, gain);
            assert!(Arc::ptr_eq(&before[0].samples, &changed[0].samples));
        }
        // A peak outside the placement must not forbid an otherwise safe gain.
        let trimmed = project
            .resize_clip(track, clip, Tick::new(0), Tick::new(7056))
            .unwrap()
            .set_clip_gain(clip, 2.0)
            .unwrap();
        let trimmed = loader(&trimmed, 48000).unwrap();
        assert_eq!(trimmed[0].samples[0], 0.25);
        assert_eq!(trimmed[0].gain, 2.0);
    }
}

#[test]
fn short_trim_cache_does_not_pin_the_unused_whole_file() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("echode-trim-cache-{}", Id::generate().raw())));
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
            AudioRef::try_new(other.hash, other.sample_rate, other.channels, other.frames).unwrap(),
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
    let quieter = prepare_media(
        &fixture.0,
        &faded.set_clip_gain(clip, 0.25).unwrap(),
        48000,
        Some(&reused),
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &prepared.clips[0].samples,
        &quieter.clips[0].samples
    ));
    assert!(Arc::ptr_eq(
        prepared.clips[0].right.as_ref().unwrap(),
        quieter.clips[0].right.as_ref().unwrap()
    ));
    assert_eq!(quieter.clips[0].gain, 0.25);
}

#[test]
fn shared_pcm_survives_placement_edits_without_rereading_media() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("echode-shared-pcm-{}", Id::generate().raw())));
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
    assert!(
        Arc::ptr_eq(&before[0].samples, &changed[0].samples),
        "clip gain copied the whole placement PCM"
    );
    assert!(Arc::ptr_eq(
        before[0].right.as_ref().unwrap(),
        changed[0].right.as_ref().unwrap()
    ));
    assert_eq!(changed[0].gain, 0.25);
    assert_eq!(changed[0].samples[0], 0.5);
    assert_eq!(changed[0].right.as_ref().unwrap()[0], -0.5);
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
            let (mut control, mut audio) =
                playback(&project, rate, block, PlaybackOptions::default()).unwrap();
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
                assert_eq!(left, expected[0].samples[source] * expected[0].gain * gain);
                assert_eq!(
                    right,
                    expected[0].right.as_ref().unwrap()[source] * expected[0].gain * gain
                );
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
            let quieter = loader(&project.set_clip_gain(clip, 0.25).unwrap(), rate).unwrap();
            assert!(Arc::ptr_eq(&quieter[0].samples, &expected[0].samples));
            assert_eq!(quieter[0].gain, 0.25);
            let faded = loader(
                &project.set_clip_fades(clip, 7_056_000, 14_112_000).unwrap(),
                rate,
            )
            .unwrap();
            assert!(Arc::ptr_eq(&faded[0].samples, &expected[0].samples));
            assert_ne!(faded[0].fade_out, expected[0].fade_out);
            let gain = project.tracks()[0].devices[0].parameters[0].id;
            let mixed = project.set_parameter(gain, 0.75).unwrap();
            let retained = loader(&mixed, rate).unwrap();
            assert!(Arc::ptr_eq(&retained[0].samples, &expected[0].samples));
            assert!(
                loader(&project, rate).is_ok(),
                "failed preparation destroyed the retained head"
            );
            // A retained unscaled whole source can rebuild geometry without I/O.
            // Rounded windows that do not publish it still need the missing file.
            let moved = project
                .resize_clip(track, clip, Tick::new(0), Tick::new(100))
                .unwrap();
            match loader(&moved, rate) {
                Ok(moved) => {
                    assert_eq!(moved[0].start_sample, 0);
                    assert!(moved[0].samples.len() < expected[0].samples.len());
                    assert!(!Arc::ptr_eq(&moved[0].samples, &expected[0].samples));
                }
                Err(PlaybackError::Media) => {}
                other => panic!("unexpected rewindow result: {other:?}"),
            }
            let slower = project.with_tempo(
                Tick::new(0),
                echode_model::Tempo::quarters_per_minute(60).unwrap(),
            );
            match loader(&slower, rate) {
                Ok(slower) => {
                    assert!(slower[0].samples.len() > expected[0].samples.len());
                    assert!(!Arc::ptr_eq(&slower[0].samples, &expected[0].samples));
                    assert_eq!(slower[0].samples.last(), Some(&0.0));
                }
                Err(PlaybackError::Media) => {}
                other => panic!("unexpected tempo result: {other:?}"),
            }
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
        assert_eq!(loaded.gain, 0.5);
        assert!((loaded.samples[mid] - 0.5).abs() < 1.0e-4);
        assert!((right[mid] + 0.5).abs() < 1.0e-4);
        let frames = loaded.start_sample as usize + loaded.samples.len();
        // Scratch sizes exercise stream partitioning, not product limits.
        for block in [1, 64, 511] {
            let (mut control, mut audio) =
                playback(&project, 48_000, block, PlaybackOptions::default()).unwrap();
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

/// PCM bytes the prepared media keeps, counting each shared plane once.
fn resident_bytes(prepared: &PreparedMedia) -> usize {
    let mut seen: Vec<Arc<[f32]>> = Vec::new();
    for planes in prepared.sources.values().chain(prepared.windows.values()) {
        for plane in std::iter::once(&planes.left).chain(planes.right.as_ref()) {
            if !seen.iter().any(|kept| Arc::ptr_eq(kept, plane)) {
                seen.push(Arc::clone(plane));
            }
        }
    }
    seen.iter()
        .map(|plane| plane.len() * size_of::<f32>())
        .sum()
}

/// EXT-01 (docs/reviews/2026-11-06-assessment-review.md): every distinct trim
/// of one source keeps its own copy of the PCM, so long overlapping trims of a
/// long take multiply its memory. One shared copy of the source would do.
#[test]
#[ignore = "EXT-01: each trim of one source copies its PCM"]
fn audit_ext01_long_trims_of_one_source_hold_one_copy_of_its_pcm() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("echode-trim-copies-{}", Id::generate().raw())));
    std::fs::create_dir_all(&fixture.0).unwrap();
    let source = fixture.0.join("take.wav");
    std::fs::write(&source, stereo_wav(48000, 48000)).unwrap();
    let stored = echode_history::import_audio(&fixture.0, &source).unwrap();
    let audio = AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let (mut project, track) = Project::new().add_track("takes");
    // Eight placements are this fixture's count, not a product limit.
    for index in 1..=8i64 {
        let clip = Id::generate();
        let start = Tick::quarters(4 * index).unwrap();
        project = project
            .place_audio(track, clip, Id::generate(), start, audio.clone())
            .unwrap();
        let length = project.tracks()[0]
            .clips
            .iter()
            .find(|placement| placement.id == clip)
            .unwrap()
            .length
            .get();
        // A different head on each placement gives each window its own key.
        let trim = index * echode_model::TICKS_PER_QUARTER / 32;
        project = project
            .resize_clip(
                track,
                clip,
                Tick::new(start.get() + trim),
                Tick::new(length - trim),
            )
            .unwrap();
    }
    let prepared = prepare_media(&fixture.0, &project, 48000, None).unwrap();
    let source_bytes = 48000 * 2 * size_of::<f32>();
    let resident = resident_bytes(&prepared);
    assert!(
        resident <= source_bytes,
        "eight trims of a {source_bytes}-byte source keep {resident} bytes of PCM"
    );
}
