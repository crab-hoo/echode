use std::path::Path;
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

fn render(package: &Path, output: &Path) -> Vec<f32> {
    let result = Command::new(env!("CARGO_BIN_EXE_echode"))
        .args([
            "render",
            package.to_str().unwrap(),
            output.to_str().unwrap(),
            "--frames",
            "1024",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    samples(&std::fs::read(output).unwrap())
}

/// A moved clip exports exactly like the same clip placed there directly, and
/// a copy at the same position is a second identical voice.
#[test]
fn a_moved_or_copied_audio_clip_exports_like_a_direct_placement() {
    let directory =
        std::env::temp_dir().join(format!("echode-clip-move-export-{}", Id::generate()));
    let (moved, direct) = (directory.join("moved"), directory.join("direct"));
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::create_dir_all(&direct).unwrap();
    let source = directory.join("source.wav");
    // This short ramp bounds this CLI fixture only, not project length.
    let ramp: Vec<f32> = (0..512).map(|index| index as f32 / 2048.0).collect();
    write_wav(&source, 48_000, &ramp).unwrap();
    let stored = import_audio(&moved, &source).unwrap();
    import_audio(&direct, &source).unwrap();
    let audio = AudioRef::try_new(
        stored.hash,
        stored.sample_rate,
        stored.channels,
        stored.frames,
    )
    .unwrap();
    // At 120 bpm a sample at 48 kHz is 29.4 ticks, so this is 100 samples.
    let landing = Tick::new(2_940);
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let clip = Id::generate();
    let original = project
        .place_audio(first, clip, Id::generate(), Tick::new(0), audio.clone())
        .unwrap()
        .set_clip_gain(clip, 0.5)
        .unwrap();
    write_package(&moved, &original).unwrap();
    let placed = Id::generate();
    let expected = project
        .place_audio(second, placed, Id::generate(), landing, audio)
        .unwrap()
        .set_clip_gain(placed, 0.5)
        .unwrap();
    write_package(&direct, &expected).unwrap();

    let host = DocumentHost::open(&moved).unwrap();
    host.perform(Edit::MoveClip {
        clip,
        track: second,
        start: landing,
    })
    .unwrap();
    drop(host);
    let reference = render(&direct, &directory.join("direct.wav"));
    assert!(reference[..200].iter().all(|sample| *sample == 0.0));
    assert!(reference.iter().any(|sample| *sample != 0.0));
    assert_eq!(render(&moved, &directory.join("moved.wav")), reference);

    let host = DocumentHost::open(&moved).unwrap();
    host.perform(Edit::DuplicateClip {
        clip,
        track: first,
        start: landing,
    })
    .unwrap();
    drop(host);
    // Two identical voices sum in f64, so the copy doubles every sample exactly.
    let doubled: Vec<f32> = reference.iter().map(|sample| sample * 2.0).collect();
    assert_eq!(render(&moved, &directory.join("copied.wav")), doubled);
    std::fs::remove_dir_all(directory).unwrap();
}
