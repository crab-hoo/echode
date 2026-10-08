use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use echode_actions::{Action, Command, Session};
use echode_history::{
    AUTOSAVE_REF, Autosave, JournalBase, read_package, recover, write_journal, write_package,
};
use echode_model::{NoteDraft, Project, TICKS_PER_QUARTER, Tick};

fn scratch() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("echode-package-{}-{n}", std::process::id()))
}

fn note(start: i64) -> NoteDraft {
    NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::quarters(start).unwrap(),
        length: Tick::new(TICKS_PER_QUARTER),
    }
}

fn with_note(name: &str) -> Project {
    let mut session = Session::open(Project::new());
    let Action::AddTrack { id: track, .. } = session
        .perform(Command::AddTrack { name: name.into() })
        .unwrap()
    else {
        panic!("add track");
    };
    let Action::InsertNote { clip, .. } = session
        .perform(Command::InsertNote {
            track,
            draft: note(1),
        })
        .unwrap()
    else {
        panic!("insert");
    };
    session
        .perform(Command::ResizeClip {
            track,
            clip,
            start: Tick::quarters(1).unwrap(),
            length: Tick::quarters(4).unwrap(),
        })
        .unwrap();
    session.project().clone()
}

#[test]
fn a_package_roundtrips_and_keeps_its_bytes() {
    let directory = scratch();
    let project = with_note("pi\"ano");
    write_package(&directory, &project).unwrap();
    assert_eq!(read_package(&directory).unwrap(), project);
    let before = std::fs::read(
        directory
            .join("tracks")
            .join(format!("{}.json", project.tracks()[0].id)),
    )
    .unwrap();
    write_package(&directory, &project).unwrap();
    let after = std::fs::read(
        directory
            .join("tracks")
            .join(format!("{}.json", project.tracks()[0].id)),
    )
    .unwrap();
    assert_eq!(before, after);
    assert!(std::str::from_utf8(&before).unwrap().contains("pi\\\"ano"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_journal_replays_onto_the_package_until_autosave_catches_up() {
    let directory = scratch();
    let saved = with_note("piano");
    write_package(&directory, &saved).unwrap();

    let mut session = Session::open(saved.clone());
    let track = saved.tracks()[0].id;
    session
        .perform(Command::InsertNote {
            track,
            draft: note(8),
        })
        .unwrap();
    write_journal(&directory, JournalBase::Files, session.journal()).unwrap();
    assert_eq!(recover(&directory).unwrap(), *session.project());

    let autosave = Autosave::create(&directory).unwrap();
    let first = autosave
        .save(Arc::new(session.project().clone()))
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(autosave.load(first).unwrap(), *session.project());
    drop(autosave);
    assert_eq!(recover(&directory).unwrap(), *session.project());

    let autosave = Autosave::create(&directory).unwrap();
    let second = autosave
        .save(Arc::new(saved.clone()))
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
    write_journal(&directory, JournalBase::Commit(first), session.journal()).unwrap();
    assert_eq!(recover(&directory).unwrap(), saved);

    let repo = gix::open(&directory).unwrap();
    let parents: Vec<_> = repo
        .find_commit(second)
        .unwrap()
        .parent_ids()
        .map(|id| id.detach())
        .collect();
    assert_eq!(parents, vec![first]);
    assert!(repo.find_reference(AUTOSAVE_REF).is_ok());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_bad_schema_is_rejected() {
    let directory = scratch();
    write_package(&directory, &with_note("piano")).unwrap();
    std::fs::write(directory.join("project.json"), "{\"schema\": 2}\n").unwrap();
    assert!(read_package(&directory).is_err());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_symlinked_track_file_is_not_followed() {
    let directory = scratch();
    let project = with_note("piano");
    write_package(&directory, &project).unwrap();
    let track = directory
        .join("tracks")
        .join(format!("{}.json", project.tracks()[0].id));
    let outside = directory.join("outside.json");
    std::fs::rename(&track, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &track).unwrap();
    let error = read_package(&directory).unwrap_err().to_string();
    let _ = std::fs::remove_dir_all(directory);
    assert!(error.contains("symbolic link"), "{error}");
}

/// HIS-1: the cache must keep the `Arc`. Address reuse is allocator-dependent,
/// so this walks the same edit twenty times.
#[test]
fn a_reused_track_address_does_not_restore_an_older_commit() {
    for _ in 0..20 {
        let directory = scratch();
        let autosave = Autosave::create(&directory).unwrap();
        let mut session = Session::open(Project::new());
        let Action::AddTrack { id: track, .. } = session
            .perform(Command::AddTrack {
                name: "piano".into(),
            })
            .unwrap()
        else {
            panic!("add track");
        };
        session
            .perform(Command::InsertNote {
                track,
                draft: note(0),
            })
            .unwrap();
        autosave
            .save(Arc::new(session.project().clone()))
            .unwrap()
            .wait()
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        session.undo().unwrap();
        session
            .perform(Command::InsertNote {
                track,
                draft: note(8),
            })
            .unwrap();
        session
            .perform(Command::InsertNote {
                track,
                draft: note(16),
            })
            .unwrap();
        let commit = autosave
            .save(Arc::new(session.project().clone()))
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(autosave.load(commit).unwrap(), *session.project());
        drop(autosave);
        let _ = std::fs::remove_dir_all(directory);
    }
}
