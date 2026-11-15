//! IQA-04: a failed save must not leave the next save trusting a stale worktree cache.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use echode_actions::{Action, Command, Session};
use echode_history::{
    AUTOSAVE_REF, Autosave, DocumentHost, GitOperation, PackageError, read_package, recover,
    write_package,
};
use echode_model::{Id, NoteDraft, ParamKey, Project, Tempo, Tick};
use gix::bstr::ByteSlice;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("echode-autosave-retry-{}", Id::generate()));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn worktree(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::from([(
        PathBuf::from("project.json"),
        fs::read(directory.join("project.json")).unwrap(),
    )]);
    for folder in ["tracks", "contents"] {
        for entry in fs::read_dir(directory.join(folder)).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.insert(
                    PathBuf::from(folder).join(path.file_name().unwrap()),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    files
}

#[derive(Clone, Copy)]
enum Failure {
    PartialWrite,
    Commit,
}

impl Failure {
    fn block(self, directory: &Path, content: Id) -> PathBuf {
        let path = match self {
            // Tracks and the earlier content have already been rewritten here.
            Self::PartialWrite => directory.join(format!("contents/{content}.json.tmp")),
            Self::Commit => directory.join(".git/refs/echode/autosave/main.lock"),
        };
        fs::create_dir(&path).unwrap();
        path
    }

    fn check(self, error: &PackageError) {
        match self {
            Self::PartialWrite => assert!(matches!(error.original(), PackageError::Io { .. })),
            Self::Commit => assert!(matches!(
                error.original(),
                PackageError::Git {
                    operation: GitOperation::Commit,
                    ..
                }
            )),
        }
        assert!(!error.is_durability_unknown());
    }
}

fn note() -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::new(100),
    }
}

fn edit(session: &mut Session) {
    let track = &session.project().tracks()[0];
    let clip = &track.clips[0];
    let content = session.project().content(clip.content).unwrap();
    let (track_id, clip_id, note_id) = (track.id, clip.id, content.notes[0].id);
    let gain = track
        .devices
        .last()
        .unwrap()
        .parameters
        .iter()
        .find(|parameter| parameter.key == ParamKey::Gain)
        .unwrap()
        .id;
    session
        .transaction(|transaction| {
            transaction.perform(Command::SetTempo {
                at: Tick::new(0),
                tempo: Tempo::quarters_per_minute(137).unwrap(),
            })?;
            transaction.perform(Command::SetParameter {
                parameter: gain,
                value: 0.25,
            })?;
            transaction.perform(Command::SetNoteVelocity {
                track: track_id,
                clip: clip_id,
                note: note_id,
                velocity: 50,
            })?;
            let Action::AddTrack { id, .. } = transaction.perform(Command::AddTrack {
                name: "only in the failed save".into(),
            })?
            else {
                unreachable!()
            };
            transaction.perform(Command::InsertNote {
                track: id,
                draft: note(),
            })?;
            Ok(())
        })
        .unwrap();
}

fn check_incremental(autosave: &Autosave, directory: &Path, target: &Project) {
    // Retaining each inode makes this independent of timestamps and inode reuse.
    let retained: Vec<_> = worktree(directory)
        .into_keys()
        .map(|path| {
            let file = fs::File::open(directory.join(&path)).unwrap();
            (path, file)
        })
        .collect();
    autosave
        .save(Arc::new(target.clone()), 3)
        .unwrap()
        .wait()
        .unwrap();
    for (path, file) in retained {
        assert_eq!(
            fs::metadata(directory.join(&path)).unwrap().ino(),
            file.metadata().unwrap().ino(),
            "a successful retry must reestablish incremental saving for {path:?}"
        );
    }
}

fn retry_after_failure(failure: Failure, undo: bool) {
    let fixture = Fixture::new();
    let directory = fixture.0.join("song.echode");
    let (base, track) = Project::new().add_track("saved");
    let base = base.insert_note(track, note()).unwrap().0;
    write_package(&directory, &base).unwrap();
    let autosave = Autosave::create(&directory).unwrap();
    let first = autosave
        .save(Arc::new(base.clone()), 0)
        .unwrap()
        .wait()
        .unwrap();
    let original = worktree(&directory);
    let chain = fs::read(directory.join(".git/echode-commit-chain")).unwrap();
    let mut session = Session::open(base.clone());
    edit(&mut session);
    let edited = session.project().clone();
    let added_track = edited.tracks().last().unwrap();
    let added_content = added_track.clips[0].content;
    let blocked = failure.block(&directory, added_content);
    let error = autosave
        .save(Arc::new(edited.clone()), 1)
        .unwrap()
        .wait()
        .unwrap_err();
    failure.check(&error);
    assert!(directory.join("save.pending").exists());
    assert_ne!(
        worktree(&directory),
        original,
        "the fault must follow actual JSON writes"
    );
    assert!(
        directory
            .join(format!("tracks/{}.json", added_track.id))
            .exists()
    );
    assert_eq!(autosave.load(first).unwrap(), base);
    assert_eq!(
        fs::read(directory.join(".git/echode-commit-chain")).unwrap(),
        chain
    );
    assert_eq!(autosave.revisions().unwrap()[0].id, first.to_string());
    fs::remove_dir(blocked).unwrap();

    let expected = if undo {
        session.undo().unwrap();
        assert_eq!(session.project(), &base);
        assert!(Arc::ptr_eq(
            &session.project().tracks()[0],
            &base.tracks()[0]
        ));
        assert!(Arc::ptr_eq(
            &session.project().contents()[0],
            &base.contents()[0]
        ));
        original
    } else {
        let expected_directory = fixture.0.join("expected.echode");
        write_package(&expected_directory, &edited).unwrap();
        worktree(&expected_directory)
    };
    let target = session.project();
    let saved = autosave
        .save(Arc::new(target.clone()), 2)
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(autosave.load(saved).unwrap(), *target);
    assert_eq!(recover(&directory).unwrap(), *target);
    assert!(!directory.join("save.pending").exists());
    assert_eq!(
        worktree(&directory),
        expected,
        "IQA-04: a successful retry must restore all worktree JSON and sweep failed-save files"
    );
    assert_eq!(read_package(&directory).unwrap(), *target);
    let repo = gix::open(&directory).unwrap();
    let commit = repo.find_commit(saved).unwrap();
    assert_eq!(commit.parent_ids().next().unwrap().detach(), first);
    assert!(
        commit
            .message_raw()
            .unwrap()
            .contains_str("Echode-Journal: 2")
    );
    assert_eq!(
        repo.find_reference(AUTOSAVE_REF).unwrap().id().detach(),
        saved
    );
    check_incremental(&autosave, &directory, target);
    drop(autosave);
    let host = DocumentHost::open(&directory).unwrap();
    assert!(host.recovery_notice().is_none());
    assert_eq!(*host.snapshot(), *target);
}

#[test]
fn audit_iqa04_partial_write_then_undo_repairs_the_worktree() {
    retry_after_failure(Failure::PartialWrite, true);
}

#[test]
fn audit_iqa04_failed_commit_then_undo_repairs_the_worktree() {
    retry_after_failure(Failure::Commit, true);
}

#[test]
fn retrying_the_edited_snapshot_after_a_partial_write_keeps_the_edit() {
    retry_after_failure(Failure::PartialWrite, false);
}

#[test]
fn retrying_the_edited_snapshot_after_a_failed_commit_keeps_the_edit() {
    retry_after_failure(Failure::Commit, false);
}
