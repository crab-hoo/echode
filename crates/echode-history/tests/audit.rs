//! Open items from docs/reviews/2026-10-08-audit.md. Each test fails until its item
//! is fixed; `make known-bugs` runs them. Remove the `#[ignore]` with the fix.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use echode_history::{Autosave, DocumentHost, read_package, recover, write_package};
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
fn audit_pro01_package_directory_links_preserve_external_files() {
    for folder in ["tracks", "contents"] {
        for has_tip in [false, true] {
            for has_note in [false, true] {
                let root = scratch();
                let directory = root.join("project.echode");
                let outside = root.join("outside");
                std::fs::create_dir_all(&outside).unwrap();
                let sentinel = outside.join("unrelated-user-file.json");
                let original = b"{\"user\":\"important\"}";
                std::fs::write(&sentinel, original).unwrap();
                let project = if has_note {
                    let (project, track) = Project::new().add_track("piano");
                    project.insert_note(track, eighth(0)).unwrap().0
                } else {
                    Project::new()
                };
                write_package(&directory, &project).unwrap();
                if has_tip {
                    let autosave = Autosave::create(&directory).unwrap();
                    autosave.save(Arc::new(project), 0).unwrap().wait().unwrap();
                }
                std::fs::remove_dir_all(directory.join(folder)).unwrap();
                std::os::unix::fs::symlink(&outside, directory.join(folder)).unwrap();
                let read = read_package(&directory);
                let recovered = recover(&directory);
                let opened = DocumentHost::open(&directory);
                let rejected = (read.is_err(), recovered.is_err(), opened.is_err());
                drop(opened);
                let preserved = std::fs::read(&sentinel).ok();
                std::fs::remove_dir_all(&root).unwrap();
                assert_eq!(
                    preserved.as_deref(),
                    Some(original.as_slice()),
                    "{folder}, tip={has_tip}, note={has_note}"
                );
                assert_eq!(
                    rejected,
                    (true, true, true),
                    "{folder}, tip={has_tip}, note={has_note}"
                );
            }
        }
    }
}

#[test]
fn audit_pro01_temporary_and_control_file_links_never_overwrite_external_files() {
    for entry in [
        "project",
        "track",
        "content",
        "ignore",
        "ignore.tmp",
        "journal.tmp",
        "pending",
        "pending.tmp",
    ] {
        let root = scratch();
        let directory = root.join("project.echode");
        let (project, track) = Project::new().add_track("piano");
        let project = project.insert_note(track, eighth(0)).unwrap().0;
        write_package(&directory, &project).unwrap();
        let sentinel = root.join("valuable.json");
        let original = b"{\"user\":\"important\"}";
        std::fs::write(&sentinel, original).unwrap();
        let path = match entry {
            "project" => directory.join("project.json.tmp"),
            "track" => directory.join("tracks").join(format!("{track}.json.tmp")),
            "content" => directory
                .join("contents")
                .join(format!("{}.json.tmp", project.contents()[0].id)),
            "ignore" => directory.join(".gitignore"),
            "ignore.tmp" => directory.join(".gitignore.tmp"),
            "journal.tmp" => directory.join("journal.jsonl.tmp"),
            "pending" => directory.join("save.pending"),
            "pending.tmp" => directory.join("save.pending.tmp"),
            _ => unreachable!(),
        };
        std::os::unix::fs::symlink(&sentinel, path).unwrap();
        let rejected = match entry {
            "ignore" | "ignore.tmp" => Autosave::create(&directory).is_err(),
            "journal.tmp" => echode_history::compact_journal(&directory, 0).is_err(),
            "pending" | "pending.tmp" => echode_history::mark_save_pending(&directory).is_err(),
            _ => write_package(&directory, &project).is_err(),
        };
        let preserved = std::fs::read(&sentinel).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(preserved, original, "{entry}");
        assert!(rejected, "{entry}");
    }
}

#[test]
fn audit_pro01_incremental_removals_preserve_external_files() {
    for folder in ["tracks", "contents"] {
        let root = scratch();
        let directory = root.join("project.echode");
        let outside = root.join("outside");
        let (project, track) = Project::new().add_track("piano");
        let project = project.insert_note(track, eighth(0)).unwrap().0;
        let filename = format!(
            "{}.json",
            if folder == "tracks" {
                track
            } else {
                project.contents()[0].id
            }
        );
        let autosave = Autosave::create(&directory).unwrap();
        autosave.save(Arc::new(project), 0).unwrap().wait().unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let original = b"external data";
        std::fs::write(outside.join(&filename), original).unwrap();
        std::fs::remove_dir_all(directory.join(folder)).unwrap();
        std::os::unix::fs::symlink(&outside, directory.join(folder)).unwrap();
        let saved = autosave.save(Arc::new(Project::new()), 1).unwrap().wait();
        drop(autosave);
        let preserved = std::fs::read(outside.join(&filename)).ok();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(preserved.as_deref(), Some(original.as_slice()), "{folder}");
        assert!(saved.is_err(), "{folder}");
    }
}

#[test]
fn audit_pro01_a_hard_linked_journal_is_not_repaired_or_appended() {
    let root = scratch();
    let directory = root.join("project.echode");
    write_package(&directory, &Project::new()).unwrap();
    let sentinel = root.join("valuable.json");
    let original = b"incomplete external JSON";
    std::fs::write(&sentinel, original).unwrap();
    std::fs::hard_link(&sentinel, directory.join("journal.jsonl")).unwrap();
    let result = echode_history::append_journal(
        &directory,
        &echode_actions::JournalLine {
            seq: 1,
            op: echode_actions::JournalOp::Undo,
            actions: Vec::new(),
            changes: Default::default(),
        },
    );
    let preserved = std::fs::read(sentinel).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(preserved, original);
    assert!(result.is_err());
}

#[test]
fn audit_pro01_an_open_package_root_replaced_with_a_link_cannot_write_outside() {
    let root = scratch();
    let directory = root.join("project.echode");
    let outside = root.join("outside");
    write_package(&directory, &Project::new()).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel.json"), b"external").unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    std::fs::rename(&directory, root.join("original.echode")).unwrap();
    std::os::unix::fs::symlink(&outside, &directory).unwrap();
    let edit = host.perform(echode_actions::Command::AddTrack {
        name: "rejected".into(),
    });
    assert!(edit.is_err());
    assert!(host.snapshot().tracks().is_empty());
    drop(host);
    assert_eq!(
        std::fs::read(outside.join("sentinel.json")).unwrap(),
        b"external"
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn audit_his2_a_torn_journal_falls_back_to_the_last_commit() {
    let directory = scratch();
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, eighth(0)).unwrap();
    write_package(&directory, &project).unwrap();
    {
        let autosave = Autosave::create(&directory).unwrap();
        autosave
            .save(Arc::new(project.clone()), 0)
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
fn audit_his3_saving_one_edited_track_rewrites_only_its_file() {
    let directory = scratch();
    let mut project = Project::new();
    let mut tracks = Vec::new();
    for name in ["drums", "bass", "keys"] {
        let (next, id) = project.add_track(name);
        project = next;
        tracks.push(id);
    }
    let (project, _) = project.insert_note(tracks[0], eighth(0)).unwrap();
    let kept_content = project.contents()[0].id;
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(Arc::new(project.clone()), 0)
        .unwrap()
        .wait()
        .unwrap();
    let modified = |id| {
        std::fs::metadata(directory.join("tracks").join(format!("{id}.json")))
            .unwrap()
            .modified()
            .unwrap()
    };
    let content_time = std::fs::metadata(
        directory
            .join("contents")
            .join(format!("{kept_content}.json")),
    )
    .unwrap()
    .modified()
    .unwrap();
    let project_time = std::fs::metadata(directory.join("project.json"))
        .unwrap()
        .modified()
        .unwrap();
    let before: Vec<_> = tracks.iter().map(|id| modified(*id)).collect();
    std::thread::sleep(Duration::from_millis(20));
    let (edited, _) = project.insert_note(tracks[1], eighth(0)).unwrap();
    autosave.save(Arc::new(edited), 0).unwrap().wait().unwrap();
    let rewritten: Vec<bool> = tracks
        .iter()
        .zip(&before)
        .map(|(id, time)| modified(*id) != *time)
        .collect();
    let content_after = std::fs::metadata(
        directory
            .join("contents")
            .join(format!("{kept_content}.json")),
    )
    .unwrap()
    .modified()
    .unwrap();
    let project_after = std::fs::metadata(directory.join("project.json"))
        .unwrap()
        .modified()
        .unwrap();
    drop(autosave);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(rewritten, vec![false, true, false]);
    assert_eq!(content_after, content_time);
    assert_ne!(project_after, project_time);
}

#[test]
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
fn audit_mod4_growing_an_auto_clip_keeps_the_other_note_lines() {
    let directory = scratch();
    let (mut project, track) = Project::new().add_track("piano");
    for index in 8..16 {
        project = project.insert_note(track, eighth(index)).unwrap().0;
    }
    write_package(&directory, &project).unwrap();
    let content = project.contents()[0].id;
    let path = directory.join("contents").join(format!("{content}.json"));
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
