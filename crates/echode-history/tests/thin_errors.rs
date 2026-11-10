use std::any::Any;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::{
    AUTOSAVE_REF, GitOperation, PackageError, SavePoint, ThinningError, pack_and_thin,
    seed_autosaves, tip_text, write_bundle,
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "echode-thin-errors-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Accept the old String result too, so the regression fails at the boundary
// assertion rather than failing to compile before the migration.
fn typed(error: &dyn Any) -> &PackageError {
    error
        .downcast_ref::<PackageError>()
        .expect("thinning lost its typed cause")
}

#[test]
fn bundle_write_preserves_the_io_cause_and_the_source_history() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let expected = std::fs::write(directory.path(), b"bundle").unwrap_err();
    let error = write_bundle(&repo, points[0].id, directory.path()).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    let PackageError::Io { error: cause, .. } = typed(&error) else {
        panic!("bundle write did not retain its I/O kind");
    };
    assert_eq!(cause.raw_os_error(), expected.raw_os_error());
    assert!(typed(&error).source().unwrap().is::<std::io::Error>());
    assert_eq!(tip_text(&repo).unwrap(), "save 0");
    assert!(repo.find_commit(points[0].id).is_ok());
}

#[test]
fn a_missing_commit_preserves_the_git_cause_without_moving_the_tip() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let missing = SavePoint {
        id: gix::ObjectId::null(repo.object_hash()),
        time: points[0].time,
    };
    let expected = repo.find_object(missing.id).unwrap_err();
    let error = pack_and_thin(&repo, &[missing], &[0]).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    assert!(matches!(
        typed(&error),
        PackageError::Git {
            operation: GitOperation::FindObject,
            ..
        }
    ));
    assert!(typed(&error).source().unwrap().is::<gix::Error>());
    assert_eq!(tip_text(&repo).unwrap(), "save 0");
    assert!(repo.find_commit(points[0].id).is_ok());
}

#[test]
fn tip_text_preserves_the_utf8_cause_and_the_original_blob() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let bytes = vec![0xff, 0xfe];
    let blob = repo
        .write_object(&gix::objs::Blob {
            data: bytes.clone(),
        })
        .unwrap()
        .detach();
    let tree = repo
        .write_object(&gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "project.json".into(),
                oid: blob,
            }],
        })
        .unwrap()
        .detach();
    let mut commit = repo
        .find_commit(points[0].id)
        .unwrap()
        .decode()
        .unwrap()
        .to_owned()
        .unwrap();
    commit.tree = tree;
    let tip = repo.write_object(&commit).unwrap().detach();
    repo.find_reference(AUTOSAVE_REF)
        .unwrap()
        .set_target_id(tip, "invalid text fixture")
        .unwrap();
    let error = tip_text(&repo).unwrap_err();
    let expected = String::from_utf8(bytes.clone()).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    let mut cause: &(dyn Error + 'static) = typed(&error);
    let text = loop {
        if let Some(text) = cause.downcast_ref::<std::string::FromUtf8Error>() {
            break text;
        }
        cause = cause.source().expect("tip text lost its UTF-8 cause");
    };
    assert_eq!(text.as_bytes(), bytes);
    assert_eq!(repo.find_blob(blob).unwrap().data, bytes);
}

#[test]
fn empty_retention_is_a_typed_refusal_without_deleting_or_moving_history() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 3, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let error = pack_and_thin(&repo, &points, &[]).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Thinning(ThinningError::NothingToKeep)
    ));
    assert_eq!(error.to_string(), "nothing to keep");
    assert!(error.source().unwrap().source().is_none());
    assert_eq!(tip_text(&repo).unwrap(), "save 2");
    for point in points {
        assert!(repo.find_commit(point.id).is_ok());
    }
}

#[test]
fn wrong_object_kind_preserves_the_git_cause_and_its_display_format() {
    let directory = Scratch::new();
    seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let blob = repo.write_blob(b"not a commit").unwrap().detach();
    let expected = repo
        .find_object(blob)
        .unwrap()
        .try_into_commit()
        .unwrap_err();
    let error = pack_and_thin(&repo, &[SavePoint { id: blob, time: 0 }], &[0]).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    assert!(matches!(
        error,
        PackageError::Git {
            operation: GitOperation::ConvertObject,
            ..
        }
    ));
    assert!(error.source().unwrap().is::<gix::Error>());
    assert_eq!(tip_text(&repo).unwrap(), "save 0");
}

#[test]
fn a_symbolic_autosave_is_not_confused_with_a_missing_project() {
    let directory = Scratch::new();
    seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let reference = repo.git_dir().join(AUTOSAVE_REF);
    let bytes = b"ref: refs/heads/another\n";
    std::fs::write(&reference, bytes).unwrap();
    let error = tip_text(&repo).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Thinning(ThinningError::SymbolicAutosave)
    ));
    assert_eq!(error.to_string(), "autosave ref is not a commit");
    assert_eq!(std::fs::read(reference).unwrap(), bytes);
}

#[test]
fn a_blocked_pack_directory_preserves_the_git_io_cause_and_history() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 2, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let reference = repo.git_dir().join(AUTOSAVE_REF);
    let original = std::fs::read(&reference).unwrap();
    let pack = repo.git_dir().join("objects/pack");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::remove_dir(&pack).unwrap();
    std::fs::write(&pack, b"occupied").unwrap();
    let expected = repo.find_object(points[0].id).unwrap_err();
    let error = pack_and_thin(&repo, &points, &[0, 1]).unwrap_err();
    assert_eq!(error.to_string(), expected.to_string());
    assert!(matches!(
        error,
        PackageError::Git {
            operation: GitOperation::FindObject,
            ..
        }
    ));
    let cause = error
        .source()
        .unwrap()
        .downcast_ref::<gix::Error>()
        .unwrap();
    assert!(
        cause.downcast_any_ref::<std::io::Error>().is_some(),
        "blocked pack directory lost its I/O cause"
    );
    assert_eq!(std::fs::read(&pack).unwrap(), b"occupied");
    assert_eq!(std::fs::read(reference).unwrap(), original);
    std::fs::remove_file(&pack).unwrap();
    std::fs::create_dir(&pack).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    assert_eq!(tip_text(&repo).unwrap(), "save 1");
    for point in points {
        assert!(repo.find_commit(point.id).is_ok());
    }
}

#[test]
fn a_missing_project_is_a_typed_refusal_without_changing_the_tree() {
    let directory = Scratch::new();
    let points = seed_autosaves(directory.path(), 1, 10, 1_700_000_000).unwrap();
    let repo = gix::open(directory.path()).unwrap();
    let tree = repo
        .write_object(gix::objs::Tree::default())
        .unwrap()
        .detach();
    let mut commit = repo
        .find_commit(points[0].id)
        .unwrap()
        .decode()
        .unwrap()
        .to_owned()
        .unwrap();
    commit.tree = tree;
    let tip = repo.write_object(&commit).unwrap().detach();
    repo.find_reference(AUTOSAVE_REF)
        .unwrap()
        .set_target_id(tip, "missing project fixture")
        .unwrap();
    let error = tip_text(&repo).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Thinning(ThinningError::MissingProject)
    ));
    assert_eq!(error.to_string(), "project.json is missing");
    assert!(
        repo.find_tree(tree)
            .unwrap()
            .decode()
            .unwrap()
            .entries
            .is_empty()
    );
}
