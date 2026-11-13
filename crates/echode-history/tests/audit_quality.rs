//! Reproductions from the 2026-11-13 implementation quality audit (IQA).
//! See docs/reviews/2026-11-13-implementation-quality-audit.md.

use std::path::{Path, PathBuf};

use echode_actions::Command;
use echode_history::{DocumentHost, write_package};
use echode_model::Project;

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir()
        .join(format!(
            "echode-audit-quality-{}-{name}",
            std::process::id()
        ))
        .join("song.echode");
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
    directory
}

fn add_track(directory: &Path, name: &str) {
    let host = DocumentHost::open(directory).unwrap();
    host.perform(Command::AddTrack { name: name.into() })
        .unwrap();
}

fn names(project: &Project) -> Vec<String> {
    project
        .tracks()
        .iter()
        .map(|track| track.name.clone())
        .collect()
}

/// The worktree's JSON files, which a save does not sync (ADR 0230).
fn worktree(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = vec![directory.join("project.json")];
    for folder in ["tracks", "contents"] {
        if let Ok(entries) = std::fs::read_dir(directory.join(folder)) {
            files.extend(entries.map(|entry| entry.unwrap().path()));
        }
    }
    files
        .into_iter()
        .map(|file| {
            let bytes = std::fs::read(&file).unwrap();
            (file, bytes)
        })
        .collect()
}

/// Puts the worktree back to `older`, as a power cut can leave it.
fn rewind_worktree(directory: &Path, older: &[(PathBuf, Vec<u8>)]) {
    for (file, _) in worktree(directory) {
        if !older.iter().any(|(kept, _)| *kept == file) {
            std::fs::remove_file(file).unwrap();
        }
    }
    for (file, bytes) in older {
        std::fs::write(file, bytes).unwrap();
    }
}

/// IQA-01: `autosave_tip` reads "the ref cannot be read" as "there is no
/// commit". Recovery then takes the unsynced worktree as the document and the
/// first save starts a new history. The commit chain still names the newest
/// commit, which holds the second track.
#[test]
#[ignore = "IQA-01: an unreadable autosave ref opens the worktree without a notice"]
fn audit_iqa01_an_unreadable_autosave_ref_does_not_silently_drop_edits() {
    let directory = scratch("ref");
    write_package(&directory, &Project::new()).unwrap();
    add_track(&directory, "first");
    let older = worktree(&directory);
    add_track(&directory, "second");
    let before = DocumentHost::open(&directory)
        .unwrap()
        .revisions()
        .unwrap()
        .len();

    rewind_worktree(&directory, &older);
    std::fs::write(directory.join(".git/refs/echode/autosave/main"), b"").unwrap();

    let host = DocumentHost::open(&directory).unwrap();
    assert!(
        host.recovery_notice().is_some() || names(&host.snapshot()) == ["first", "second"],
        "opened {:?} without a notice",
        names(&host.snapshot())
    );
    assert!(
        host.revisions().unwrap().len() >= before,
        "the history fell from {before} revisions to {}",
        host.revisions().unwrap().len()
    );
    drop(host);
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

/// IQA-02: `open_or_init` answers every failed open with `gix::init`, so a
/// repository that exists but cannot be read is reported as a failed
/// initialization and the cause of the open is lost.
#[test]
#[ignore = "IQA-02: a repository that cannot be opened is reported as a failed init"]
fn audit_iqa02_an_unreadable_repository_reports_why_it_did_not_open() {
    let directory = scratch("config");
    write_package(&directory, &Project::new()).unwrap();
    add_track(&directory, "first");

    std::fs::write(directory.join(".git/config"), b"[core\n\0").unwrap();

    let error = match DocumentHost::open(&directory) {
        Ok(_) => panic!("a package with an unreadable repository opened"),
        Err(error) => format!("{error:?}"),
    };
    assert!(
        !error.contains("Init"),
        "the open failure is reported as an initialization: {error}"
    );
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}
