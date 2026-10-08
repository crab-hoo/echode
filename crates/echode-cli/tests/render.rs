use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_cli::render_to_wav;
use echode_history::write_package;
use echode_model::{NoteDraft, Project, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("echode-render-{}-{n}", std::process::id()))
}

#[test]
fn a_package_renders_to_the_same_wav_twice() {
    let directory = scratch();
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 69,
                velocity: 127,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap();
    write_package(&directory, &project).unwrap();

    let first = directory.join("first.wav");
    let second = directory.join("second.wav");
    render_to_wav(&directory, &first, 48_000, Some(128)).unwrap();
    render_to_wav(&directory, &second, 48_000, Some(128)).unwrap();
    let first_bytes = std::fs::read(&first).unwrap();
    let second_bytes = std::fs::read(&second).unwrap();
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(&first_bytes[0..4], b"RIFF");
    assert_eq!(&first_bytes[8..12], b"WAVE");
    assert_eq!(&first_bytes[20..22], &3u16.to_le_bytes());
    assert!(first_bytes.len() > 44);
    assert!(first_bytes[44..].iter().any(|byte| *byte != 0));
    let _ = std::fs::remove_dir_all(directory);
}
