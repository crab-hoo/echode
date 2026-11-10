use std::any::Any;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use echode_history::spike::SpikeError;
use echode_history::spike::{History, Project};
use echode_history::{GitOperation, PackageError};
use gix::objs::tree::{Entry, EntryKind};

struct Fixture {
    history: History,
    repo: gix::Repository,
    directory: PathBuf,
    first: gix::ObjectId,
}

impl Fixture {
    fn new() -> Self {
        let directory = super::scratch();
        let history = History::create(&directory).unwrap();
        let first = history
            .save(Arc::new(Project { tracks: Vec::new() }))
            .unwrap()
            .wait()
            .unwrap();
        let repo = gix::open(&directory).unwrap();
        Self {
            history,
            repo,
            directory,
            first,
        }
    }

    fn revision(&self, project: &[u8], tracks: Vec<Entry>) -> gix::ObjectId {
        let project = self.repo.write_blob(project).unwrap().detach();
        let tracks = self
            .repo
            .write_object(gix::objs::Tree { entries: tracks })
            .unwrap()
            .detach();
        self.revision_with_entries(vec![
            Entry {
                mode: EntryKind::Blob.into(),
                filename: "project.json".into(),
                oid: project,
            },
            Entry {
                mode: EntryKind::Tree.into(),
                filename: "tracks".into(),
                oid: tracks,
            },
        ])
    }

    fn revision_with_entries(&self, entries: Vec<Entry>) -> gix::ObjectId {
        let tree = self
            .repo
            .write_object(gix::objs::Tree { entries })
            .unwrap()
            .detach();
        let mut commit = self
            .repo
            .find_commit(self.first)
            .unwrap()
            .decode()
            .unwrap()
            .to_owned()
            .unwrap();
        commit.tree = tree;
        self.repo.write_object(&commit).unwrap().detach()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

// Accept String too, so the pre-migration failure is an assertion, not a build error.
fn typed(error: &dyn Any) -> &PackageError {
    error
        .downcast_ref::<PackageError>()
        .expect("history spike lost its typed cause")
}

#[test]
fn missing_entries_and_wrong_object_kinds_keep_their_causes() {
    let fixture = Fixture::new();
    let missing = fixture.revision_with_entries(Vec::new());
    let error = fixture.history.load(missing).unwrap_err();
    assert!(matches!(error, PackageError::MissingRevisionProject));
    assert_eq!(
        error.to_string(),
        "project.json is missing from the revision"
    );

    let project = fixture
        .repo
        .write_blob(br#"{"schema":1,"tracks":[]}"#)
        .unwrap()
        .detach();
    let missing = fixture.revision_with_entries(vec![Entry {
        mode: EntryKind::Blob.into(),
        filename: "project.json".into(),
        oid: project,
    }]);
    let error = fixture.history.load(missing).unwrap_err();
    assert!(matches!(&error, PackageError::MissingRevisionDirectory(name) if name == "tracks"));
    assert_eq!(
        error.to_string(),
        "tracks directory is missing from the revision"
    );

    let tree = fixture
        .repo
        .write_object(gix::objs::Tree {
            entries: Vec::new(),
        })
        .unwrap()
        .detach();
    let wrong_kind = fixture.revision_with_entries(vec![Entry {
        mode: EntryKind::Tree.into(),
        filename: "project.json".into(),
        oid: tree,
    }]);
    let expected = fixture
        .repo
        .find_object(tree)
        .unwrap()
        .try_into_blob()
        .err()
        .unwrap();
    let error = fixture.history.load(wrong_kind).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Git {
            operation: GitOperation::ObjectKind,
            ..
        }
    ));
    assert!(error.source().unwrap().is::<gix::Error>());
    assert_eq!(error.to_string(), format!("{expected:?}"));
    assert!(fixture.history.load(fixture.first).is_ok());
}

#[test]
fn a_missing_revision_retains_the_git_operation_and_cause() {
    let fixture = Fixture::new();
    let missing = gix::ObjectId::null(fixture.repo.object_hash());
    let expected = fixture.repo.find_commit(missing).unwrap_err();
    let error = fixture.history.load(missing).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    assert!(matches!(
        typed(&error),
        PackageError::Git {
            operation: GitOperation::FindObject,
            ..
        }
    ));
    assert!(typed(&error).source().unwrap().is::<gix::Error>());
    assert!(fixture.history.load(fixture.first).is_ok());
}

#[test]
fn malformed_revision_json_retains_the_json_cause() {
    let fixture = Fixture::new();
    let revision = fixture.revision(b"{", Vec::new());
    let error = fixture.history.load(revision).unwrap_err();
    let cause = typed(&error).source().unwrap().source().unwrap();
    let json = cause.downcast_ref::<serde_json::Error>().unwrap();
    assert!(json.is_eof());
    assert_eq!(error.to_string(), format!("project.json: {json}"));
    assert_eq!(fixture.repo.find_commit(revision).unwrap().id, revision);
    assert!(fixture.history.load(fixture.first).is_ok());
}

#[test]
fn a_failed_save_retains_errno_without_advancing_the_tip() {
    let fixture = Fixture::new();
    let tracks = fixture.directory.join("tracks");
    std::fs::remove_dir(&tracks).unwrap();
    std::fs::write(&tracks, b"blocked").unwrap();
    let expected = std::fs::create_dir_all(&tracks).unwrap_err();
    let error = fixture
        .history
        .save(Arc::new(Project { tracks: Vec::new() }))
        .unwrap()
        .wait()
        .unwrap_err();
    let PackageError::Io { error: cause, .. } = typed(&error) else {
        panic!("spike save did not retain its I/O cause");
    };
    assert_eq!(cause.raw_os_error(), expected.raw_os_error());
    assert_eq!(error.to_string(), expected.to_string());
    assert_eq!(fixture.repo.head_id().unwrap().detach(), fixture.first);
    assert_eq!(std::fs::read(tracks).unwrap(), b"blocked");
}

#[test]
fn schema_rejections_keep_the_target_and_version() {
    let fixture = Fixture::new();
    let revision = fixture.revision(br#"{"schema":4294967295,"tracks":[]}"#, Vec::new());
    let error = fixture.history.load(revision).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Spike(SpikeError::ProjectSchema(u32::MAX))
    ));
    assert_eq!(error.to_string(), "unsupported project schema 4294967295");
    assert!(error.source().unwrap().source().is_none());

    let track = fixture
        .repo
        .write_blob(br#"{"schema":2,"id":"t","name":"t","gain_milli_db":0,"notes":[]}"#)
        .unwrap()
        .detach();
    let revision = fixture.revision(
        br#"{"schema":1,"tracks":["t"]}"#,
        vec![Entry {
            mode: EntryKind::Blob.into(),
            filename: "t.json".into(),
            oid: track,
        }],
    );
    let error = fixture.history.load(revision).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Spike(SpikeError::TrackSchema(2))
    ));
    assert_eq!(error.to_string(), "unsupported track schema 2");
    assert!(fixture.history.load(fixture.first).is_ok());
}

#[test]
fn track_json_utf8_and_missing_files_remain_distinct() {
    let fixture = Fixture::new();
    let project = br#"{"schema":1,"tracks":["t"]}"#;
    let revision = fixture.revision(project, Vec::new());
    let error = fixture.history.load(revision).unwrap_err();
    assert!(matches!(&error, PackageError::MissingRevisionFile(path) if path == "tracks/t.json"));
    assert_eq!(error.to_string(), "tracks/t.json is missing");

    for data in [b"{".as_slice(), &[0xff]] {
        let blob = fixture.repo.write_blob(data).unwrap().detach();
        let revision = fixture.revision(
            project,
            vec![Entry {
                mode: EntryKind::Blob.into(),
                filename: "t.json".into(),
                oid: blob,
            }],
        );
        let error = fixture.history.load(revision).unwrap_err();
        if data == b"{" {
            let PackageError::Spike(SpikeError::TrackJson(json)) = &error else {
                panic!("track JSON cause was not retained");
            };
            assert!(json.is_eof());
            assert_eq!(error.to_string(), format!("track json: {json}"));
            assert!(
                error
                    .source()
                    .unwrap()
                    .source()
                    .unwrap()
                    .is::<serde_json::Error>()
            );
        } else {
            let PackageError::RevisionText(cause) = &error else {
                panic!("UTF-8 cause was not retained");
            };
            assert_eq!(cause.valid_up_to(), 0);
            assert!(error.source().unwrap().is::<std::str::Utf8Error>());
        }
        assert_eq!(fixture.repo.find_blob(blob).unwrap().data, data);
    }
    assert!(fixture.history.load(fixture.first).is_ok());
}

#[test]
fn an_initialization_failure_does_not_deadlock_shutdown() {
    const CHILD: &str = "ECHODE_FAILED_SPIKE_HISTORY";
    if let Some(directory) = std::env::var_os(CHILD) {
        let history = History::create(std::path::Path::new(&directory)).unwrap();
        let error = history
            .save(Arc::new(Project { tracks: Vec::new() }))
            .unwrap()
            .wait()
            .unwrap_err();
        // Drop must not wait forever for the failed worker to accept shutdown.
        let later = history
            .load(gix::ObjectId::null(gix::hash::Kind::Sha1))
            .unwrap_err();
        let (PackageError::Shared(first), PackageError::Shared(second)) =
            (typed(&error), typed(&later))
        else {
            panic!("initialization cause was not shared");
        };
        assert!(Arc::ptr_eq(first, second));
        drop(history);
        assert!(matches!(
            typed(&error).original(),
            PackageError::Git {
                operation: GitOperation::Init,
                ..
            }
        ));
        return;
    }
    let directory = super::scratch();
    std::fs::write(&directory, b"not a directory").unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD, &directory)
        .args([
            "--exact",
            "errors::an_initialization_failure_does_not_deadlock_shutdown",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Bound a deadlock, not a performance measurement. No UI or hardware is opened.
    let deadline = Instant::now() + Duration::from_secs(10);
    let completed = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    std::fs::remove_file(directory).unwrap();
    assert!(
        completed,
        "failed spike history initialization deadlocked during shutdown"
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
