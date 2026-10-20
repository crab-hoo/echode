use std::process::Command;

use echode_actions::Command as Edit;
use echode_cli::write_wav;
use echode_history::{DocumentHost, import_audio, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

fn samples(bytes: &[u8]) -> Vec<f32> {
    bytes[44..]
        .chunks_exact(4)
        .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()))
        .collect()
}

#[test]
fn a_duplicated_audio_track_exports_as_a_second_identical_voice() {
    let directory =
        std::env::temp_dir().join(format!("echode-duplicate-export-{}", Id::generate()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("source.wav");
    // This short ramp bounds this CLI fixture only, not project length.
    let ramp: Vec<f32> = (0..512).map(|index| index as f32 / 2048.0).collect();
    write_wav(&source, 48_000, &ramp).unwrap();
    let stored = import_audio(&directory, &source).unwrap();
    let (project, track) = Project::new().add_track("source");
    let clip = Id::generate();
    let audio = AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    let project = project
        .place_audio(track, clip, Id::generate(), Tick::new(0), audio)
        .unwrap()
        .set_clip_gain(clip, 0.5)
        .unwrap();
    write_package(&directory, &project).unwrap();
    let render = |name: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_echode"))
            .args([
                "render",
                directory.to_str().unwrap(),
                directory.join(name).to_str().unwrap(),
                "--frames",
                "256",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        samples(&std::fs::read(directory.join(name)).unwrap())
    };
    let single = render("single.wav");
    assert!(single.iter().any(|sample| *sample != 0.0));
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Edit::DuplicateTrack { source: track })
        .unwrap();
    drop(host);
    // Two identical voices sum in f64, so the copy doubles every sample exactly.
    let doubled: Vec<f32> = single.iter().map(|sample| sample * 2.0).collect();
    assert_eq!(render("duplicated.wav"), doubled);
    std::fs::remove_dir_all(directory).unwrap();
}
