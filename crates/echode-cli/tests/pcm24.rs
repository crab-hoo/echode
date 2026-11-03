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
            "echode-pcm24-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn render(package: &Path, wav: &Path, rate: u32, frames: u32, format: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_echode"));
    command
        .arg("render")
        .arg(package)
        .arg(wav)
        .args(["--sample-rate", &rate.to_string()])
        .args(["--frames", &frames.to_string()]);
    if let Some(format) = format {
        command.args(["--format", format]);
    }
    command.output().unwrap()
}

fn assert_pcm24(bytes: &[u8], rate: u32, frames: u32) -> Vec<i32> {
    let format = support::chunk(bytes, b"fmt ");
    assert_eq!(format.len(), 40);
    assert_eq!(u16::from_le_bytes(format[0..2].try_into().unwrap()), 0xfffe);
    assert_eq!(u16::from_le_bytes(format[2..4].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(format[4..8].try_into().unwrap()), rate);
    assert_eq!(
        u32::from_le_bytes(format[8..12].try_into().unwrap()),
        rate * 6
    );
    assert_eq!(u16::from_le_bytes(format[12..14].try_into().unwrap()), 6);
    assert_eq!(u16::from_le_bytes(format[14..16].try_into().unwrap()), 24);
    assert_eq!(u16::from_le_bytes(format[16..18].try_into().unwrap()), 22);
    assert_eq!(u16::from_le_bytes(format[18..20].try_into().unwrap()), 24);
    assert_eq!(u32::from_le_bytes(format[20..24].try_into().unwrap()), 3);
    assert_eq!(
        &format[24..40],
        &[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71
        ]
    );
    let data = support::chunk(bytes, b"data");
    assert_eq!(data.len(), frames as usize * 6);
    data.chunks_exact(3)
        .map(|sample| i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8)
        .collect()
}

#[test]
fn pcm24_cli_exports_stereo_silence_at_multiple_rates_and_block_boundaries() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = directory.0.join("out.wav");
    for rate in [44_100, 48_000, 96_000] {
        // These lengths exercise empty, short, full, and partial writer blocks only.
        for frames in [0, 1, 64, 65, 513] {
            let output = render(&directory.0, &wav, rate, frames, Some("pcm24"));
            assert!(
                output.status.success(),
                "PCM24 CLI export failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let bytes = std::fs::read(&wav).unwrap();
            assert_eq!(bytes.len(), 68 + frames as usize * 6);
            assert!(
                assert_pcm24(&bytes, rate, frames)
                    .iter()
                    .all(|value| *value == 0)
            );
        }
    }
}

#[test]
fn pcm24_cli_quantizes_the_same_placed_audio_as_float_without_wrapping() {
    let directory = Scratch::new();
    let source = directory.0.join("source.wav");
    // Different channel values catch swapped channels as well as byte/sign errors.
    let samples: Vec<_> = (0..257).flat_map(|_| [-2.0, 0.25, -0.5, 2.0]).collect();
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
    compare_formats(&directory.0, &project, 514);
}

#[test]
fn pcm24_cli_uses_the_same_isolated_instrument_output_as_float() {
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
    let output = render(&directory.0, &empty, 48_000, 0, Some("pcm24"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(assert_pcm24(&std::fs::read(empty).unwrap(), 48_000, 0).is_empty());
    compare_formats(&directory.0, &project, 513);
}

fn compare_formats(package: &Path, project: &Project, frames: u32) {
    write_package(package, project).unwrap();
    let float = package.join("float.wav");
    let pcm = package.join("pcm.wav");
    for (wav, format) in [(&float, None), (&pcm, Some("pcm24"))] {
        let output = render(package, wav, 48_000, frames, format);
        assert!(
            output.status.success(),
            "format {format:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("exported "));
    }
    let float = std::fs::read(float).unwrap();
    let pcm = std::fs::read(pcm).unwrap();
    let decoded = assert_pcm24(&pcm, 48_000, frames);
    assert!(decoded.iter().any(|value| *value != 0));
    for (sample, integer) in support::chunk(&float, b"data")
        .chunks_exact(4)
        .zip(&decoded)
    {
        let sample = f32::from_le_bytes(sample.try_into().unwrap());
        let actual = f64::from(*integer) / 8_388_608.0;
        let expected = f64::from(sample).clamp(-1.0, 8_388_607.0 / 8_388_608.0);
        assert!(
            (actual - expected).abs() <= 0.5 / 8_388_608.0,
            "PCM24 sample differs from float: {actual} != {expected}"
        );
    }
    let expected: Vec<_> = decoded
        .iter()
        .map(|sample| *sample as f32 / 8_388_608.0)
        .collect();
    // The product's independent CoreAudio file reader must accept the container.
    let decoded = echode_history::decode_audio_file(&package.join("pcm.wav"), &|| true).unwrap();
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.frames, u64::from(frames));
    assert_eq!(decoded.samples, expected);
}

#[test]
fn pcm24_cli_rejects_invalid_options_and_headers_without_replacing_output() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = directory.0.join("keep.wav");
    std::fs::write(&wav, b"keep").unwrap();
    for (rate, frames, format, message) in [
        (48_000, 1, "pcm32", "expected wav format"),
        (48_000, 1, "", "expected wav format"),
        (0, 1, "pcm24", "invalid sample rate"),
        (u32::MAX / 6 + 1, 0, "pcm24", "sample rate does not fit"),
        (
            u32::MAX / 6 + 1,
            (u32::MAX - 60) / 6 + 1,
            "pcm24",
            "sample rate does not fit",
        ),
    ] {
        let output = render(&directory.0, &wav, rate, frames, Some(format));
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(!stderr.contains("exported "));
        assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_echode"))
        .arg("render")
        .arg(&directory.0)
        .arg(&wav)
        .arg("--format")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
    assert!(std::fs::read_dir(&directory.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")
    }));
}

#[test]
fn explicit_float32_keeps_the_default_wav_bytes() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let default = directory.0.join("default.wav");
    let explicit = directory.0.join("explicit.wav");
    for (wav, format) in [(&default, None), (&explicit, Some("float32"))] {
        let output = render(&directory.0, wav, 48_000, 65, format);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        std::fs::read(default).unwrap(),
        std::fs::read(explicit).unwrap()
    );
}
