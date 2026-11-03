mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::{import_audio, write_package};
use echode_model::{AudioRef, Id, NoteDraft, Project, Tick};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "echode-pcm16-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn render(package: &Path, wav: &Path, rate: u32, frames: u32, format: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_echode"))
        .arg("render")
        .arg(package)
        .arg(wav)
        .args(["--sample-rate", &rate.to_string()])
        .args(["--frames", &frames.to_string()])
        .args(["--format", format])
        .output()
        .unwrap()
}

fn assert_pcm16(bytes: &[u8], rate: u32, frames: u32) -> Vec<i16> {
    // Canonical stereo PCM uses a 16-byte fmt, no extension or fact chunk.
    assert_eq!(bytes.len(), 44 + frames as usize * 4);
    let format = support::chunk(bytes, b"fmt ");
    assert_eq!(format.len(), 16);
    assert_eq!(u16::from_le_bytes(format[0..2].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(format[2..4].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(format[4..8].try_into().unwrap()), rate);
    assert_eq!(
        u32::from_le_bytes(format[8..12].try_into().unwrap()),
        rate * 4
    );
    assert_eq!(u16::from_le_bytes(format[12..14].try_into().unwrap()), 4);
    assert_eq!(u16::from_le_bytes(format[14..16].try_into().unwrap()), 16);
    let data = support::chunk(bytes, b"data");
    assert_eq!(data.len(), frames as usize * 4);
    data.chunks_exact(2)
        .map(|sample| i16::from_le_bytes(sample.try_into().unwrap()))
        .collect()
}

#[test]
fn pcm16_cli_exports_stereo_silence_at_multiple_rates_and_block_boundaries() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = directory.0.join("out.wav");
    for rate in [44_100, 48_000, 96_000] {
        // These lengths exercise empty, short, full, and partial writer blocks only.
        for frames in [0, 1, 64, 65, 513] {
            let output = render(&directory.0, &wav, rate, frames, "pcm16");
            assert!(
                output.status.success(),
                "PCM16 CLI export failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("exported "));
            let bytes = std::fs::read(&wav).unwrap();
            assert!(
                assert_pcm16(&bytes, rate, frames)
                    .iter()
                    .all(|value| *value == 0)
            );
        }
    }
}

#[test]
fn pcm16_cli_quantizes_placed_stereo_audio_and_roundtrips_through_the_product_reader() {
    let directory = Scratch::new();
    let source = directory.0.join("source.wav");
    // Asymmetric channels, over-range audio, and sub-LSB values expose byte/sign errors.
    let samples: Vec<_> = (0..257)
        .flat_map(|_| [-2.0, 0.25, -0.5, 2.0, -0.5 / 32_768.0, 1.5 / 32_768.0])
        .collect();
    echode_cli::write_wav(&source, 48_000, &samples).unwrap();
    let stored = import_audio(&directory.0, &source).unwrap();
    let (project, track) = Project::new().add_track("stereo");
    let project = project
        .place_audio(
            track,
            Id::generate(),
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
    compare_with_float(&directory.0, &project, 771);
}

#[test]
fn pcm16_cli_quantizes_the_same_isolated_instrument_output_as_float() {
    let directory = Scratch::new();
    let (project, track) = Project::new().add_track("instrument");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap()
        .insert_note(
            track,
            NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0;
    write_package(&directory.0, &project).unwrap();
    let empty = directory.0.join("empty.wav");
    let output = render(&directory.0, &empty, 48_000, 0, "pcm16");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(assert_pcm16(&std::fs::read(empty).unwrap(), 48_000, 0).is_empty());
    compare_with_float(&directory.0, &project, 513);
}

fn compare_with_float(package: &Path, project: &Project, frames: u32) {
    write_package(package, project).unwrap();
    let float = package.join("float.wav");
    let pcm = package.join("pcm.wav");
    for (wav, format) in [(&float, "float32"), (&pcm, "pcm16")] {
        let output = render(package, wav, 48_000, frames, format);
        assert!(
            output.status.success(),
            "format {format}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("exported "));
    }
    let float = std::fs::read(float).unwrap();
    let bytes = std::fs::read(&pcm).unwrap();
    let integers = assert_pcm16(&bytes, 48_000, frames);
    assert!(integers.iter().any(|value| *value != 0));
    for (sample, integer) in support::chunk(&float, b"data")
        .chunks_exact(4)
        .zip(&integers)
    {
        let sample = f32::from_le_bytes(sample.try_into().unwrap());
        let actual = f64::from(*integer) / 32_768.0;
        let expected = f64::from(sample).clamp(-1.0, 32_767.0 / 32_768.0);
        assert!(
            (actual - expected).abs() <= 0.5 / 32_768.0,
            "PCM16 sample differs from float: {actual} != {expected}"
        );
    }
    // Independent AudioToolbox decoding checks compatibility, not just our own parser.
    let decoded = echode_history::decode_audio_file(&pcm, &|| true).unwrap();
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.frames, u64::from(frames));
    let expected: Vec<_> = integers
        .iter()
        .map(|sample| f32::from(*sample) / 32_768.0)
        .collect();
    assert_eq!(decoded.samples, expected);
}

#[test]
fn pcm16_cli_rejects_invalid_headers_without_replacing_output() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = directory.0.join("keep.wav");
    std::fs::write(&wav, b"keep").unwrap();
    for (rate, frames, message) in [
        (0, 1, "invalid sample rate"),
        (u32::MAX / 4 + 1, 0, "sample rate does not fit"),
        (48_000, (u32::MAX - 36) / 4 + 1, "wav data does not fit"),
        (48_000, u32::MAX, "wav data does not fit"),
    ] {
        let output = render(&directory.0, &wav, rate, frames, "pcm16");
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(!stderr.contains("exported "));
        assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
    }
    assert!(std::fs::read_dir(&directory.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")
    }));
}
