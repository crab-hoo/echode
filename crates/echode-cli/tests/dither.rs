#[path = "support/output.rs"]
mod output;
mod support;
use output::output_path;

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
            "echode-dither-{}-{}",
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

fn render(package: &Path, wav: &Path, frames: u32, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_echode"))
        .arg("render")
        .arg(package)
        .arg(wav)
        .args(["--frames", &frames.to_string()])
        .args(flags)
        .output()
        .unwrap()
}

fn integers(bytes: &[u8], bits: u16) -> Vec<i32> {
    support::chunk(bytes, b"data")
        .chunks_exact(usize::from(bits / 8))
        .map(|sample| match bits {
            16 => i32::from(i16::from_le_bytes(sample.try_into().unwrap())),
            24 => i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8,
            _ => unreachable!(),
        })
        .collect()
}

fn successful_bytes(package: &Path, wav: &Path, frames: u32, flags: &[&str]) -> Vec<u8> {
    let output = render(package, wav, frames, flags);
    assert!(
        output.status.success(),
        "TPDF CLI export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("exported "));
    std::fs::read(wav).unwrap()
}

#[test]
fn dither_cli_reproducible_tpdf_at_both_integer_depths() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = output_path("out.wav");
    for (format, bits) in [("pcm16", 16), ("pcm24", 24)] {
        let flags = ["--format", format, "--dither", "tpdf"];
        // This population bounds the deterministic distribution check, not exports.
        let reference = successful_bytes(&directory.0, &wav, 16_385, &flags);
        let values = integers(&reference, bits);
        assert!(values.iter().all(|value| (-1..=1).contains(value)));
        let mean = values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64;
        let power = values
            .iter()
            .map(|value| f64::from(value * value))
            .sum::<f64>()
            / values.len() as f64;
        assert!(mean.abs() < 0.02, "TPDF silence mean: {mean}");
        assert!((0.23..0.27).contains(&power), "TPDF silence power: {power}");
        assert!(values.chunks_exact(2).any(|frame| frame[0] != frame[1]));
        assert_eq!(
            successful_bytes(&directory.0, &wav, 16_385, &flags),
            reference,
            "repeated exports must use the same seed"
        );
        // Empty, short, and partial writer blocks must keep the same prefix.
        for frames in [0, 1, 65, 513] {
            let bytes = successful_bytes(&directory.0, &wav, frames, &flags);
            assert_eq!(integers(&bytes, bits), values[..frames as usize * 2]);
        }
        let decoded = echode_history::decode_audio_file(&wav, &|| true).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.frames, 513);
        let scale = (1u32 << (bits - 1)) as f32;
        let expected: Vec<_> = values[..1026]
            .iter()
            .map(|value| *value as f32 / scale)
            .collect();
        assert_eq!(decoded.samples, expected);
    }
}

#[test]
fn explicit_none_preserves_default_bytes_for_all_formats() {
    let directory = Scratch::new();
    write_package(&directory.0, &Project::new()).unwrap();
    let wav = output_path("out.wav");
    for format in ["float32", "pcm16", "pcm24"] {
        let default = successful_bytes(&directory.0, &wav, 513, &["--format", format]);
        let explicit = successful_bytes(
            &directory.0,
            &wav,
            513,
            &["--format", format, "--dither", "none"],
        );
        assert_eq!(default, explicit);
    }
}

#[test]
fn tpdf_quantizes_the_same_placed_audio_and_isolated_instrument_as_float() {
    let directory = Scratch::new();
    let source = directory.0.join("source.wav");
    // Asymmetric channels include over-range, negative, and sub-LSB values.
    let samples: Vec<_> = (0..257)
        .flat_map(|_| [-2.0, 0.25, -0.000_001, 2.0])
        .collect();
    echode_cli::write_wav(&source, 48_000, &samples).unwrap();
    let stored = import_audio(&directory.0, &source).unwrap();
    let (project, track) = Project::new().add_track("placed");
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
    compare_to_float(&directory.0, &project);

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
    compare_to_float(&directory.0, &project);
}

fn compare_to_float(package: &Path, project: &Project) {
    write_package(package, project).unwrap();
    let wav = output_path("out.wav");
    let float = successful_bytes(package, &wav, 513, &["--format", "float32"]);
    let samples: Vec<_> = support::chunk(&float, b"data")
        .chunks_exact(4)
        .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
        .collect();
    assert!(samples.iter().any(|sample| sample.abs() > 0.01));
    for (format, bits) in [("pcm16", 16), ("pcm24", 24)] {
        let plain = successful_bytes(package, &wav, 513, &["--format", format]);
        let dithered = successful_bytes(
            package,
            &wav,
            513,
            &["--format", format, "--dither", "tpdf"],
        );
        assert_ne!(
            support::chunk(&plain, b"data"),
            support::chunk(&dithered, b"data")
        );
        assert_eq!(
            support::chunk(&plain, b"fmt "),
            support::chunk(&dithered, b"fmt ")
        );
        let scale = f64::from(1u32 << (bits - 1));
        for (integer, sample) in integers(&dithered, bits).iter().zip(&samples) {
            let target = (f64::from(*sample) * scale).clamp(-scale, scale - 1.0);
            assert!(
                (f64::from(*integer) - target).abs() <= 1.5,
                "TPDF error exceeds 1.5 LSB"
            );
        }
    }
}

#[test]
fn invalid_dither_is_rejected_before_opening_the_project_or_replacing_output() {
    let directory = Scratch::new();
    let missing = directory.0.join("missing.echode");
    let wav = directory.0.join("keep.wav");
    std::fs::write(&wav, b"keep").unwrap();
    for (flags, message) in [
        (vec!["--dither", "other"], "expected dither none or tpdf"),
        (vec!["--dither", ""], "expected dither none or tpdf"),
        (vec!["--dither"], "usage:"),
        (
            vec!["--dither", "tpdf"],
            "--dither tpdf requires pcm16 or pcm24",
        ),
        (
            vec!["--dither", "tpdf", "--format", "float32"],
            "--dither tpdf requires pcm16 or pcm24",
        ),
    ] {
        let output = render(&missing, &wav, 1, &flags);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(!stderr.contains("exported "));
        assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    }
}
