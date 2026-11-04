use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::write_package;
use echode_model::Project;
use serde_json::Value;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "echode-progress-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        write_package(&root.join("Song.echode"), &Project::new()).unwrap();
        Self(root)
    }

    fn command(&self, output: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_echode"));
        command
            .arg("render")
            .arg(self.0.join("Song.echode"))
            .arg(output);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn events(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("progress must be NDJSON only"))
        .collect()
}

#[test]
fn progress_is_opt_in_monotonic_includes_tail_and_preserves_wav_bytes() {
    let fixture = Fixture::new();
    let plain = fixture.0.join("plain.wav");
    let output = fixture
        .command(&plain)
        .args(["--tail-frames", "257"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let tracked = fixture.0.join("tracked.wav");
    let output = fixture
        .command(&tracked)
        .args(["--tail-frames", "257", "--progress-json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "progress export failed: {output:?}"
    );
    assert_eq!(
        std::fs::read(plain).unwrap(),
        std::fs::read(tracked).unwrap()
    );
    let events = events(&output);
    assert_eq!(events.first().unwrap()["phase"], "rendering");
    assert_eq!(events.first().unwrap()["rendered_frames"], 0);
    assert_eq!(events.last().unwrap()["phase"], "complete");
    assert_eq!(events.last().unwrap()["rendered_frames"], 257);
    assert_eq!(events[events.len() - 2]["phase"], "finalizing");
    assert!(events.len() <= 103);
    let mut previous = 0;
    for event in events {
        assert_eq!(event["version"], 1);
        assert_eq!(event["total_frames"], 257);
        let frames = event["rendered_frames"].as_u64().unwrap();
        assert!(frames >= previous && frames <= 257);
        previous = frames;
    }
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("exported ")
    );
}

#[test]
fn empty_export_finishes_but_failed_publication_never_reports_complete() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.0.join("empty.wav"))
        .args(["--frames", "0", "--progress-json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "empty progress export failed: {output:?}"
    );
    let empty = events(&output);
    assert_eq!(empty.len(), 3);
    assert_eq!(empty.last().unwrap()["phase"], "complete");
    assert!(empty.iter().all(|event| event["total_frames"] == 0));
    let blocked = fixture.0.join("blocked.wav");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), b"original").unwrap();
    let output = fixture
        .command(&blocked)
        .args(["--frames", "65", "--progress-json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let failed = events(&output);
    assert_eq!(failed.last().unwrap()["phase"], "finalizing");
    assert!(!failed.iter().any(|event| event["phase"] == "complete"));
    assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"original");
    assert!(
        !String::from_utf8(output.stderr)
            .unwrap()
            .contains("exported ")
    );
}

#[test]
fn a_closed_progress_reader_does_not_cancel_or_fail_the_export() {
    let fixture = Fixture::new();
    let output = fixture.0.join("unobserved.wav");
    let mut child = fixture
        .command(&output)
        .args(["--frames", "6400", "--progress-json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "progress is advisory: {result:?}");
    assert!(output.exists());
}

#[test]
fn isolated_plugin_progress_counts_the_automatic_tail_and_partial_last_block() {
    use echode_model::{Id, NoteDraft, Tick};

    let fixture = Fixture::new();
    let (project, track) = Project::new().add_track("Sine Pad");
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
    write_package(&fixture.0.join("Song.echode"), &project).unwrap();
    let binaries = Path::new(env!("CARGO_BIN_EXE_echode")).parent().unwrap();
    let cache = fixture.0.join("plugins.jsonl");
    let entry = serde_json::json!({
        "path": binaries.join("libechode_sine_pad.dylib"),
        "bundle_version": "", "modified_secs": 0, "modified_nanos": 0, "executable_size": 0,
        "plugins": [{"id": "dev.echode.sine-pad", "name": "Sine Pad", "vendor": "Echode",
            "version": "0.0.0", "features": ["instrument"], "url": ""}]
    });
    std::fs::write(&cache, format!("{entry}\n")).unwrap();
    let output = fixture
        .command(&fixture.0.join("plugin.wav"))
        .env("ECHODE_PLUGIN_HOST", binaries.join("echode-plugin-host"))
        .env("ECHODE_PLUGIN_CACHE", cache)
        .arg("--progress-json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "plugin progress failed: {output:?}"
    );
    let events = events(&output);
    // 24,000 score frames + the fixture's 19,200-frame tail; not a product limit.
    assert!(events.iter().all(|event| event["total_frames"] == 43_200));
    assert_eq!(events.last().unwrap()["rendered_frames"], 43_200);
    assert_eq!(events.last().unwrap()["phase"], "complete");
    assert_eq!(events[events.len() - 2]["phase"], "finalizing");
    assert!(events.len() <= 103);
}
