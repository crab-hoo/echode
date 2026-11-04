#[path = "support/output.rs"]
mod output;
mod support;
use output::output_path;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use echode_actions::Command as Edit;
use echode_history::{DocumentHost, import_audio, write_package};
use echode_model::{AudioRef, Id, NoteDraft, Project, Tick};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("echode-export-tail-{}", Id::generate()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn render(package: &Path, wav: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_echode"))
        .arg("render")
        .arg(package)
        .arg(wav)
        .args(args)
        .output()
        .unwrap()
}

fn exported(package: &Path, wav: &Path, args: &[&str]) -> Vec<u8> {
    let output = render(package, wav, args);
    assert!(
        output.status.success(),
        "explicit tail export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("exported "));
    std::fs::read(wav).unwrap()
}

fn instrument_project(with_note: bool) -> Project {
    let (project, track) = Project::new().add_track("instrument");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap();
    if !with_note {
        return project;
    }
    project
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
        .0
}

#[test]
fn explicit_tails_render_empty_projects_in_every_wav_format() {
    let directory = Scratch::new();
    for (index, project) in [Project::new(), instrument_project(false)]
        .into_iter()
        .enumerate()
    {
        let package = directory.0.join(index.to_string());
        write_package(&package, &project).unwrap();
        let wav = output_path("tail.wav");
        for (format, frame_bytes) in [("float32", 8), ("pcm16", 4), ("pcm24", 6)] {
            // These lengths bound this fixture only and cross the plugin callback boundary.
            for tail in [0, 257] {
                let bytes = exported(
                    &package,
                    &wav,
                    &["--format", format, "--tail-frames", &tail.to_string()],
                );
                let data = support::chunk(&bytes, b"data");
                assert_eq!(data.len(), tail * frame_bytes);
                assert!(data.iter().all(|byte| *byte == 0));
            }
        }
    }
}

#[test]
fn explicit_tails_replace_the_device_tail_without_changing_the_rendered_samples() {
    let directory = Scratch::new();
    let project = instrument_project(true);
    write_package(&directory.0, &project).unwrap();
    let wav = output_path("tail.wav");
    for rate in [44_100, 48_000] {
        let rate_text = rate.to_string();
        let body = echode_engine::render_span_frames(&project, rate).unwrap();
        let automatic = exported(&directory.0, &wav, &["--sample-rate", &rate_text]);
        assert_eq!(
            support::chunk(&automatic, b"data").len(),
            (body + rate * 400 / 1000) as usize * 8
        );
        // Shorter, longer, and zero tails exercise replacement rather than addition.
        for tail in [0, 257, 20_001] {
            let bytes = exported(
                &directory.0,
                &wav,
                &[
                    "--sample-rate",
                    &rate_text,
                    "--tail-frames",
                    &tail.to_string(),
                ],
            );
            let total = body + tail;
            let exact = exported(
                &directory.0,
                &wav,
                &["--sample-rate", &rate_text, "--frames", &total.to_string()],
            );
            assert_eq!(
                bytes, exact,
                "explicit tail must match an exact-length export"
            );
            let data = support::chunk(&bytes, b"data");
            assert_eq!(data.len(), total as usize * 8);
            let shared = data.len().min(support::chunk(&automatic, b"data").len());
            assert_eq!(
                &data[..shared],
                &support::chunk(&automatic, b"data")[..shared]
            );
            if tail > 0 {
                assert!(data[body as usize * 8..].iter().any(|byte| *byte != 0));
            }
        }
    }
}

#[test]
fn a_placed_audio_export_appends_exactly_the_requested_silence() {
    let directory = Scratch::new();
    let source = directory.0.join("source.wav");
    // Only this fixture is limited to 257 asymmetric stereo frames.
    echode_cli::write_wav(&source, 48_000, &[0.25, -0.5].repeat(257)).unwrap();
    let stored = import_audio(&directory.0, &source).unwrap();
    let (project, track) = Project::new().add_track("audio");
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
    write_package(&directory.0, &project).unwrap();
    let wav = output_path("tail.wav");
    let automatic = exported(&directory.0, &wav, &[]);
    let body = support::chunk(&automatic, b"data");
    assert!(body.iter().any(|byte| *byte != 0));
    let bytes = exported(&directory.0, &wav, &["--tail-frames", "65"]);
    let data = support::chunk(&bytes, b"data");
    assert_eq!(data.len(), body.len() + 65 * 8);
    assert_eq!(&data[..body.len()], body);
    assert!(data[body.len()..].iter().all(|byte| *byte == 0));
    let decoded = echode_history::decode_audio_file(&wav, &|| true).unwrap();
    assert_eq!(decoded.frames, (body.len() / 8 + 65) as u64);
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 2);
}

#[test]
fn explicit_tails_use_the_recovered_edit_end_without_rewriting_the_project() {
    let directory = Scratch::new();
    let (project, track) = Project::new().add_track("journal");
    write_package(&directory.0, &project).unwrap();
    let host = DocumentHost::open(&directory.0).unwrap();
    host.perform(Edit::InsertNote {
        track,
        draft: NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::quarters(1).unwrap(),
            length: Tick::quarters(1).unwrap(),
        },
    })
    .unwrap();
    let snapshot = host.snapshot();
    let body = echode_engine::render_span_frames(&snapshot, 48_000).unwrap();
    drop(host);
    let recovered = DocumentHost::open(&directory.0).unwrap();
    let (revision, seq) = recovered.recovered_label().unwrap();
    assert_eq!(*recovered.snapshot(), *snapshot);
    drop(recovered);
    let wav = output_path("journal.wav");
    let output = render(&directory.0, &wav, &["--tail-frames", "65"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!("exported {revision} {seq}"))
    );
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(
        support::chunk(&bytes, b"data").len(),
        (body + 65) as usize * 8
    );
    let reopened = DocumentHost::open(&directory.0).unwrap();
    assert_eq!(*reopened.snapshot(), *snapshot);
    assert_eq!(reopened.recovered_label().unwrap(), (revision, seq));
}

#[test]
fn invalid_or_conflicting_tail_lengths_preserve_the_destination() {
    let directory = Scratch::new();
    write_package(&directory.0, &instrument_project(true)).unwrap();
    let wav = output_path("keep.wav");
    std::fs::write(&wav, b"keep").unwrap();
    for (args, expected) in [
        (vec!["--tail-frames"], "usage:"),
        (vec!["--tail-frames", "-1"], "non-negative integer"),
        (vec!["--tail-frames", "1.5"], "non-negative integer"),
        (vec!["--tail-frames", "NaN"], "non-negative integer"),
        (vec!["--tail-frames", "4294967296"], "non-negative integer"),
        (
            vec!["--tail-frames", "0", "--frames", "0"],
            "cannot combine",
        ),
        (
            vec!["--frames", "1", "--tail-frames", "1"],
            "cannot combine",
        ),
        (
            vec!["--tail-frames", "4294967295"],
            "export length overflowed",
        ),
        (
            vec!["--tail-frames", "0", "--sample-rate", "0"],
            "invalid sample rate",
        ),
    ] {
        let output = render(&directory.0, &wav, &args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
        assert!(!stderr.contains("exported "));
        assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
        assert!(
            std::fs::read_dir(wav.parent().unwrap())
                .unwrap()
                .all(|entry| {
                    !entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".part")
                })
        );
    }
}
