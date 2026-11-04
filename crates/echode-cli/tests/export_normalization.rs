mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::write_package;
use echode_model::{AudioRef, Id, Project, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "echode-normalization-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn audio_package(root: &Path) -> PathBuf {
    // This fixture crosses render blocks and puts the loudest sample near the end.
    let mut samples = vec![0.125f32; 1025 * 2];
    for pair in samples.chunks_exact_mut(2) {
        pair[1] = -0.0625;
    }
    samples[2000] = 0.25;
    let source = root.join("source.wav");
    echode_cli::write_wav(&source, 48_000, &samples).unwrap();
    let package = root.join("song.echode");
    let stored = echode_history::import_audio(&package, &source).unwrap();
    let (project, track) = Project::new().add_track("Stereo");
    let audio = AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let project = project
        .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
        .unwrap();
    write_package(&package, &project).unwrap();
    package
}

fn samples(bytes: &[u8], format: &str) -> Vec<f64> {
    let data = support::chunk(bytes, b"data");
    match format {
        "float32" => data
            .chunks_exact(4)
            .map(|sample| f64::from(f32::from_le_bytes(sample.try_into().unwrap())))
            .collect(),
        "pcm16" => data
            .chunks_exact(2)
            .map(|sample| f64::from(i16::from_le_bytes(sample.try_into().unwrap())) / 32768.0)
            .collect(),
        "pcm24" => data
            .chunks_exact(3)
            .map(|sample| {
                let word = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
                f64::from(word) / 8388608.0
            })
            .collect(),
        _ => unreachable!(),
    }
}

#[test]
fn contract_peak_normalization_uses_the_whole_stereo_mix_before_quantization() {
    let root = scratch();
    let package = audio_package(&root);
    let baseline = root.join("baseline.wav");
    echode_cli::render_to_wav(&package, &baseline, 48_000, Some(1025)).unwrap();
    let original = samples(&std::fs::read(&baseline).unwrap(), "float32");
    let peak = original.iter().copied().map(f64::abs).fold(0.0, f64::max);
    assert!(peak > 0.0);
    for format in ["float32", "pcm16", "pcm24"] {
        for target_db in ["-6", "0", "-120"] {
            let output = root.join(format!("{format}-{target_db}.wav"));
            let result = echode_cli::run([
                "render",
                package.to_str().unwrap(),
                output.to_str().unwrap(),
                "--frames",
                "1025",
                "--format",
                format,
                "--normalize-peak-db",
                target_db,
            ]);
            assert!(
                result.is_ok(),
                "peak-normalized CLI export was rejected: {result:?}"
            );
            let normalized = samples(&std::fs::read(&output).unwrap(), format);
            let target = 10.0f64.powf(target_db.parse::<f64>().unwrap() / 20.0);
            let tolerance = match format {
                "pcm16" => 1.0 / 32768.0,
                "pcm24" => 1.0 / 8388608.0,
                _ => 1e-7,
            };
            assert_eq!(normalized.len(), original.len());
            for (actual, original) in normalized.iter().zip(&original) {
                assert!(
                    (actual - original * target / peak).abs() <= tolerance,
                    "normalization must apply one gain to every sample, including both channels"
                );
            }
        }
    }
    assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_normalization_preserves_the_destination_and_silence_stays_silent() {
    let root = scratch();
    let package = root.join("empty.echode");
    write_package(&package, &Project::new()).unwrap();
    let output = root.join("out.wav");
    for value in ["NaN", "inf", "-inf", "1", "-120.1", "bad"] {
        std::fs::write(&output, b"keep original").unwrap();
        let error = echode_cli::run([
            "render",
            package.to_str().unwrap(),
            output.to_str().unwrap(),
            "--frames",
            "1",
            "--normalize-peak-db",
            value,
        ])
        .unwrap_err();
        assert!(error.to_string().contains("peak target"), "{error}");
        assert_eq!(std::fs::read(&output).unwrap(), b"keep original");
    }
    for frames in ["0", "129"] {
        echode_cli::run([
            "render",
            package.to_str().unwrap(),
            output.to_str().unwrap(),
            "--frames",
            frames,
            "--normalize-peak-db",
            "-1",
        ])
        .unwrap();
        assert!(
            samples(&std::fs::read(&output).unwrap(), "float32")
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_instrument_exports_are_normalized_after_their_declared_tail() {
    let root = scratch();
    let (project, track) = Project::new().add_track("Sine Pad");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap()
        .insert_note(
            track,
            echode_model::NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0;
    let package = root.join("instrument.echode");
    write_package(&package, &project).unwrap();
    let output = root.join("normalized.wav");
    echode_cli::run([
        "render",
        package.to_str().unwrap(),
        output.to_str().unwrap(),
        "--normalize-peak-db",
        "-3",
    ])
    .unwrap();
    let rendered = samples(&std::fs::read(output).unwrap(), "float32");
    assert_eq!(rendered.len(), (24_000 + 19_200) * 2);
    let peak = rendered.into_iter().map(f64::abs).fold(0.0, f64::max);
    assert!((peak - 10.0f64.powf(-3.0 / 20.0)).abs() < 1e-7);
    std::fs::remove_dir_all(root).unwrap();
}
