//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use echode_history::{Autosave, read_package, recover, write_package};
use echode_model::{NoteDraft, Project, TICKS_PER_QUARTER, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-audit-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn eighth(index: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(index * TICKS_PER_QUARTER / 2),
        length: Tick::new(TICKS_PER_QUARTER / 2),
    }
}

#[test]
#[ignore = "HIS-2: a damaged journal makes recover fail even with a good commit"]
fn audit_his2_a_torn_journal_falls_back_to_the_last_commit() {
    let directory = scratch();
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, eighth(0)).unwrap();
    write_package(&directory, &project).unwrap();
    {
        let autosave = Autosave::create(&directory).unwrap();
        autosave
            .save(Arc::new(project.clone()))
            .unwrap()
            .wait()
            .unwrap();
    }
    std::fs::write(directory.join("journal.jsonl"), b"{\"base\":\"").unwrap();
    let recovered = recover(&directory);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(recovered.unwrap(), project);
}

#[test]
#[ignore = "HIS-3: every autosave rewrites every track file"]
fn audit_his3_saving_one_edited_track_rewrites_only_its_file() {
    let directory = scratch();
    let mut project = Project::new();
    let mut tracks = Vec::new();
    for name in ["drums", "bass", "keys"] {
        let (next, id) = project.add_track(name);
        project = next;
        tracks.push(id);
    }
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(Arc::new(project.clone()))
        .unwrap()
        .wait()
        .unwrap();
    let modified = |id| {
        std::fs::metadata(directory.join("tracks").join(format!("{id}.json")))
            .unwrap()
            .modified()
            .unwrap()
    };
    let before: Vec<_> = tracks.iter().map(|id| modified(*id)).collect();
    std::thread::sleep(Duration::from_millis(20));
    let (edited, _) = project.insert_note(tracks[1], eighth(0)).unwrap();
    autosave.save(Arc::new(edited)).unwrap().wait().unwrap();
    let rewritten: Vec<bool> = tracks
        .iter()
        .zip(&before)
        .map(|(id, time)| modified(*id) != *time)
        .collect();
    drop(autosave);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(rewritten, vec![false, true, false]);
}

#[test]
#[ignore = "HIS-8: track entries in project.json become paths without validation"]
fn audit_his8_a_track_entry_must_be_an_id_before_it_becomes_a_path() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    std::fs::create_dir_all(directory.join("outside.json")).unwrap();
    std::fs::write(
        directory.join("project.json"),
        "{\"schema\": 1, \"meter\": {\"numerator\": 4, \"denominator\": 4}, \
         \"tempo\": [{\"at\": 0, \"numerator\": 120, \"denominator\": 1}], \
         \"tracks\": [\"../outside\"]}\n",
    )
    .unwrap();
    let error = read_package(&directory).unwrap_err().to_string();
    let _ = std::fs::remove_dir_all(&directory);
    // The entry must be rejected as an id, not followed to tracks/../outside.json.
    assert!(!error.contains("outside.json"), "{error}");
}

#[test]
#[ignore = "MOD-4: growing an auto clip rewrites the position of every note in it"]
fn audit_mod4_growing_an_auto_clip_keeps_the_other_note_lines() {
    let directory = scratch();
    let (mut project, track) = Project::new().add_track("piano");
    for index in 8..16 {
        project = project.insert_note(track, eighth(index)).unwrap().0;
    }
    write_package(&directory, &project).unwrap();
    let path = directory.join("tracks").join(format!("{track}.json"));
    let before = std::fs::read_to_string(&path).unwrap();
    let project = project.insert_note(track, eighth(7)).unwrap().0;
    write_package(&directory, &project).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    let note_lines = |text: &str| {
        text.lines()
            .filter(|line| line.contains("\"pitch\""))
            .map(|line| line.trim().trim_end_matches(',').to_string())
            .collect::<Vec<_>>()
    };
    let after_lines = note_lines(&after);
    for line in note_lines(&before) {
        assert!(
            after_lines.contains(&line),
            "this note line changed: {line}"
        );
    }
}
