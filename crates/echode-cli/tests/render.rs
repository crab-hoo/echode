use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use echode_cli::{render_to_wav, write_wav};
use echode_history::write_package;
use echode_model::{NoteDraft, Project, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("echode-render-{}-{n}", std::process::id()))
}

fn reference_project() -> Project {
    let (project, track) = Project::new().add_track("piano");
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

fn reference_wav() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine-pad-a4-128.wav")
}

fn render_reference(directory: &std::path::Path) -> Vec<u8> {
    write_package(directory, &reference_project()).unwrap();
    let wav = directory.join("out.wav");
    render_to_wav(directory, &wav, 48_000, Some(128)).unwrap();
    std::fs::read(wav).unwrap()
}

#[test]
fn a_package_renders_to_the_committed_reference() {
    let directory = scratch();
    let rendered = render_reference(&directory);
    let reference = std::fs::read(reference_wav()).unwrap_or_else(|_| {
        panic!(
            "missing {}; run make bless REASON=\"why the sound changed\"",
            reference_wav().display()
        )
    });
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(&rendered[0..4], b"RIFF");
    assert_eq!(&rendered[8..12], b"WAVE");
    assert_eq!(&rendered[20..22], &3u16.to_le_bytes());
    assert!(rendered.len() > 44);
    assert!(rendered[44..].iter().any(|byte| *byte != 0));
    assert_eq!(rendered, reference);
}

#[test]
#[ignore = "timing measurement; make spike-render runs it in a release build"]
fn a_minute_of_stereo_silence_is_written_in_bulk() {
    let directory = scratch();
    std::fs::create_dir_all(&directory).unwrap();
    let wav = directory.join("minute.wav");
    let samples = vec![0.0f32; 48_000 * 60 * 2];
    let started = Instant::now();
    write_wav(&wav, 48_000, &samples).unwrap();
    let elapsed = started.elapsed();
    let bytes = std::fs::metadata(&wav).unwrap().len();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(bytes, 44 + samples.len() as u64 * 4);
    // A write of one sample at a time took 15.3 s. Buffered writes of this
    // buffer were 0.028 s. Two seconds still fails the old path.
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "60 s stereo write took {elapsed:?}"
    );
}

/// Rewrites the committed reference. `make bless REASON=...` sets `BLESS=1`.
#[test]
#[ignore = "rewrites tests/fixtures/sine-pad-a4-128.wav; use make bless"]
fn bless_the_reference() {
    assert_eq!(
        std::env::var("BLESS").ok().as_deref(),
        Some("1"),
        "set BLESS=1 via make bless REASON=\"...\""
    );
    let directory = scratch();
    let rendered = render_reference(&directory);
    let path = reference_wav();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &rendered).unwrap();
    let _ = std::fs::remove_dir_all(directory);
}
