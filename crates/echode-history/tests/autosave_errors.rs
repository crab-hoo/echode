//! Autosave replies retain the original storage and Git failures.

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use echode_history::{Autosave, GitOperation, PackageError};
use echode_model::{Id, Project};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-autosave-errors-{}", Id::generate()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cause<T: Error + 'static>(error: &PackageError) -> Option<&T> {
    let mut current = error.source();
    while let Some(source) = current {
        if let Some(found) = source.downcast_ref::<T>() {
            return Some(found);
        }
        current = source.source();
    }
    None
}

#[test]
fn save_replies_retain_the_storage_cause() {
    let fixture = Fixture::new();
    let autosave = Autosave::create(&fixture.0).unwrap();
    autosave.revisions().unwrap();
    fs::write(fixture.0.join("tracks"), b"not a directory").unwrap();
    let project = Arc::new(Project::new());
    let first = autosave.save(Arc::clone(&project), 0).unwrap();
    let second = autosave.save(project, 0).unwrap();
    for pending in [first, second] {
        let error = pending.wait().unwrap_err();
        assert!(
            cause::<std::io::Error>(&error).is_some(),
            "autosave reply lost its I/O cause: {error:?}"
        );
        assert!(matches!(error.original(), PackageError::Io { .. }));
        assert!(!error.is_durability_unknown());
    }
    assert_eq!(
        fs::read(fixture.0.join("tracks")).unwrap(),
        b"not a directory"
    );
}

#[test]
fn initialization_failure_reaches_later_requests_with_its_git_cause() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join(".git")).unwrap();
    fs::write(fixture.0.join(".git/config"), b"not a config").unwrap();
    let autosave = Autosave::create(&fixture.0).unwrap();
    for _ in 0..2 {
        let error = autosave.revisions().unwrap_err();
        assert!(
            cause::<gix::Error>(&error).is_some(),
            "autosave initialization lost its Git cause: {error:?}"
        );
        assert!(matches!(
            error.original(),
            PackageError::Git {
                operation: GitOperation::Init,
                ..
            }
        ));
    }
    assert_eq!(
        fs::read(fixture.0.join(".git/config")).unwrap(),
        b"not a config"
    );
}

#[test]
fn revision_load_replies_retain_the_git_cause() {
    let fixture = Fixture::new();
    let autosave = Autosave::create(&fixture.0).unwrap();
    let revision = gix::ObjectId::null(gix::hash::Kind::Sha1);
    let error = autosave.load(revision).unwrap_err();
    assert!(
        cause::<gix::Error>(&error).is_some(),
        "autosave revision load lost its Git cause: {error:?}"
    );
    assert!(matches!(
        error.original(),
        PackageError::Git {
            operation: GitOperation::FindObject,
            ..
        }
    ));
}

#[test]
fn commit_failure_reaches_save_waiters_with_its_git_cause() {
    let fixture = Fixture::new();
    let autosave = Autosave::create(&fixture.0).unwrap();
    autosave.revisions().unwrap();
    fs::write(fixture.0.join(".git/refs/echode"), b"blocked").unwrap();
    let error = autosave
        .save(Arc::new(Project::new()), 0)
        .unwrap()
        .wait()
        .unwrap_err();
    assert!(cause::<gix::Error>(&error).is_some());
    assert!(matches!(
        error.original(),
        PackageError::Git {
            operation: GitOperation::Commit,
            ..
        }
    ));
    assert!(!error.is_durability_unknown());
    assert_eq!(
        fs::read(fixture.0.join(".git/refs/echode")).unwrap(),
        b"blocked"
    );
}

fn entry(
    name: &str,
    kind: gix::objs::tree::EntryKind,
    oid: gix::ObjectId,
) -> gix::objs::tree::Entry {
    gix::objs::tree::Entry {
        mode: kind.into(),
        filename: name.into(),
        oid,
    }
}

fn revision(repo: &gix::Repository, mut entries: Vec<gix::objs::tree::Entry>) -> gix::ObjectId {
    entries.sort();
    let tree = repo
        .write_object(gix::objs::Tree { entries })
        .unwrap()
        .detach();
    let signature = gix::actor::Signature {
        name: "Test".into(),
        email: "test@echode.local".into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    repo.commit_as(
        signature.to_ref(&mut committer_time),
        signature.to_ref(&mut author_time),
        format!("refs/tests/{}", Id::generate()),
        "malformed revision fixture",
        tree,
        std::iter::empty::<gix::ObjectId>(),
    )
    .unwrap()
    .detach()
}

#[test]
fn malformed_revision_entries_keep_their_failure_kinds() {
    use gix::objs::tree::EntryKind::{Blob, Tree};
    let fixture = Fixture::new();
    let (project, track) = Project::new().add_track("Track");
    echode_history::write_package(&fixture.0, &project).unwrap();
    let autosave = Autosave::create(&fixture.0).unwrap();
    autosave.revisions().unwrap();
    let repo = gix::open(&fixture.0).unwrap();
    let empty = repo
        .write_object(gix::objs::Tree { entries: vec![] })
        .unwrap()
        .detach();
    let json = fs::read(fixture.0.join("project.json")).unwrap();
    let project_blob = repo.write_blob(json).unwrap().detach();
    let invalid_text = repo.write_blob([0xff]).unwrap().detach();
    let cases = [
        (vec![], PackageError::MissingRevisionProject),
        (
            vec![entry("project.json", Blob, project_blob)],
            PackageError::MissingRevisionDirectory("tracks".into()),
        ),
        (
            vec![
                entry("project.json", Blob, project_blob),
                entry("tracks", Tree, empty),
            ],
            PackageError::MissingRevisionFile(format!("tracks/{track}.json")),
        ),
    ];
    for (entries, expected) in cases {
        let error = autosave.load(revision(&repo, entries)).unwrap_err();
        assert_eq!(
            std::mem::discriminant(error.original()),
            std::mem::discriminant(&expected)
        );
        assert_eq!(error.to_string(), expected.to_string());
    }
    let error = autosave
        .load(revision(
            &repo,
            vec![entry("project.json", Blob, invalid_text)],
        ))
        .unwrap_err();
    assert!(matches!(error.original(), PackageError::RevisionText(_)));
    assert!(cause::<std::str::Utf8Error>(&error).is_some());
    let error = autosave
        .load(revision(&repo, vec![entry("project.json", Tree, empty)]))
        .unwrap_err();
    let PackageError::Git {
        operation: GitOperation::ObjectKind,
        error: native,
    } = error.original()
    else {
        panic!("wrong revision object kind lost its cause: {error:?}");
    };
    assert_eq!(error.to_string(), format!("{native:?}"));
}
