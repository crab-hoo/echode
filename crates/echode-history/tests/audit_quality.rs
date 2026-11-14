//! Regressions from the 2026-11-13 implementation quality audit (IQA).
//! See docs/reviews/2026-11-13-implementation-quality-audit.md.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use echode_actions::Command;
use echode_history::{
    Autosave, DocumentError, DocumentHost, GitOperation, PackageError, recover, write_package,
};
use echode_model::{Id, Project};

const REFERENCE: &str = ".git/refs/echode/autosave/main";
const CHAIN: &str = ".git/echode-commit-chain";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir()
            .join(format!("echode-audit-quality-{}", Id::generate()))
            .join("song.echode");
        write_package(&directory, &Project::new()).unwrap();
        Self(directory.canonicalize().unwrap())
    }

    fn saved() -> (Self, Vec<(PathBuf, Vec<u8>)>, usize) {
        let fixture = Self::new();
        add_track(&fixture.0, "first");
        let older = worktree(&fixture.0);
        add_track(&fixture.0, "second");
        let count = DocumentHost::open(&fixture.0)
            .unwrap()
            .revisions()
            .unwrap()
            .len();
        (fixture, older, count)
    }

    fn reopen(&self, count: usize) {
        let host =
            DocumentHost::open(&self.0).expect("a failed open must release the package lock");
        assert!(host.recovery_notice().is_none());
        assert_eq!(names(&host.snapshot()), ["first", "second"]);
        assert_eq!(host.revisions().unwrap().len(), count);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
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

fn bytes(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![directory.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                folders.push(path);
            } else {
                files.insert(
                    path.strip_prefix(directory).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    files
}

fn refuses_without_writing(directory: &Path, check: impl Fn(&PackageError)) {
    let before = bytes(directory);
    check(&recover(directory).unwrap_err());
    for _ in 0..2 {
        let error = DocumentHost::open(directory)
            .err()
            .expect("recovery must refuse unreadable history instead of opening the worktree");
        check(&error);
    }
    let autosave = Autosave::create(directory).unwrap();
    check(&autosave.revisions().unwrap_err());
    check(
        &autosave
            .save(Arc::new(Project::new()), 0)
            .unwrap()
            .wait()
            .unwrap_err(),
    );
    drop(autosave);
    assert_eq!(bytes(directory), before, "refusal changed the package");
}

fn git_failure(error: &PackageError, expected: GitOperation) {
    assert!(
        matches!(error.original(), PackageError::Git { operation, .. } if *operation == expected),
        "expected {expected:?}, got {error:?}"
    );
    assert!(error.original().source().is_some(), "lost the Git cause");
}

/// IQA-01: an unreadable ref must never start a new history from stale JSON.
#[test]
fn audit_iqa01_an_unreadable_autosave_ref_does_not_silently_drop_edits() {
    for broken in [b"".as_slice(), b"not a commit\n"] {
        let (fixture, older, count) = Fixture::saved();
        let reference = std::fs::read(fixture.0.join(REFERENCE)).unwrap();
        rewind_worktree(&fixture.0, &older);
        std::fs::write(fixture.0.join(REFERENCE), broken).unwrap();
        refuses_without_writing(&fixture.0, |error| {
            git_failure(error, GitOperation::FindReference);
        });
        // Repair only the test's injected damage; both confirmed edits still exist.
        std::fs::write(fixture.0.join(REFERENCE), reference).unwrap();
        fixture.reopen(count);
    }
}

#[test]
fn a_missing_ref_with_a_recorded_chain_is_not_a_new_history() {
    for pending in [false, true] {
        let (fixture, older, count) = Fixture::saved();
        let reference = std::fs::read(fixture.0.join(REFERENCE)).unwrap();
        rewind_worktree(&fixture.0, &older);
        std::fs::remove_file(fixture.0.join(REFERENCE)).unwrap();
        if pending {
            std::fs::write(fixture.0.join("save.pending"), b"pending").unwrap();
        }
        refuses_without_writing(&fixture.0, |error| {
            assert!(matches!(
                error.original(),
                PackageError::Document(DocumentError::MissingAutosaveReference)
            ));
        });
        std::fs::write(fixture.0.join(REFERENCE), reference).unwrap();
        fixture.reopen(count);
    }
}

#[test]
fn a_symbolic_autosave_ref_is_refused_instead_of_panicking() {
    let (fixture, older, count) = Fixture::saved();
    let reference = std::fs::read(fixture.0.join(REFERENCE)).unwrap();
    rewind_worktree(&fixture.0, &older);
    std::fs::write(fixture.0.join(REFERENCE), b"ref: refs/heads/missing\n").unwrap();
    refuses_without_writing(&fixture.0, |error| {
        assert!(matches!(
            error.original(),
            PackageError::Document(DocumentError::SymbolicAutosaveReference)
        ));
    });
    std::fs::write(fixture.0.join(REFERENCE), reference).unwrap();
    fixture.reopen(count);
}

#[test]
fn a_missing_ref_does_not_hide_an_unreadable_chain() {
    let (fixture, _, _) = Fixture::saved();
    std::fs::remove_file(fixture.0.join(REFERENCE)).unwrap();
    std::fs::remove_file(fixture.0.join(CHAIN)).unwrap();
    std::fs::create_dir(fixture.0.join(CHAIN)).unwrap();
    refuses_without_writing(&fixture.0, |error| {
        assert!(matches!(
            error.original(),
            PackageError::Io { path: Some(path), .. } if path == &fixture.0.join(CHAIN)
        ));
    });
}

/// IQA-02: opening existing Git metadata must retain the open failure, not init.
#[test]
fn audit_iqa02_an_unreadable_repository_reports_why_it_did_not_open() {
    let (fixture, older, count) = Fixture::saved();
    let config = std::fs::read(fixture.0.join(".git/config")).unwrap();
    rewind_worktree(&fixture.0, &older);
    std::fs::write(fixture.0.join(".git/config"), b"[core\n\0").unwrap();
    refuses_without_writing(&fixture.0, |error| git_failure(error, GitOperation::Open));
    std::fs::write(fixture.0.join(".git/config"), config).unwrap();
    fixture.reopen(count);
}

#[test]
fn new_packages_do_not_race_their_background_git_initialization() {
    // Exercise concurrent first opens, not a product limit or a timing benchmark.
    std::thread::scope(|scope| {
        for _ in 0..32 {
            scope.spawn(|| {
                let fixture = Fixture::new();
                let host = DocumentHost::open(&fixture.0)
                    .expect("a new package must wait for its initial autosave");
                assert_eq!(host.revisions().unwrap().len(), 1);
            });
        }
    });
}

#[test]
fn a_new_package_and_an_empty_repository_can_start_their_first_history() {
    for initialized in [false, true] {
        let fixture = Fixture::new();
        if initialized {
            gix::init(&fixture.0).unwrap();
        }
        add_track(&fixture.0, "first");
        let host = DocumentHost::open(&fixture.0).unwrap();
        assert_eq!(names(&host.snapshot()), ["first"]);
        assert_eq!(host.revisions().unwrap().len(), 2);
        assert!(host.recovery_notice().is_none());
    }
}
