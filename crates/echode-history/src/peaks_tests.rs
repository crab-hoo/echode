use super::*;

#[test]
fn audit_pro01_peak_cache_directory_links_are_not_followed() {
    for folder in ["cache", "peaks"] {
        let root =
            std::env::temp_dir().join(format!("echode-peak-links-{}-{folder}", std::process::id()));
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
