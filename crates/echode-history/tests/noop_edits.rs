//! No-op edits must not publish, consume redo, or request an autosave.

use std::path::PathBuf;
use std::sync::Arc;

use echode_actions::{Action, Command, Session};
use echode_history::{DocumentHost, write_package};
use echode_model::{Id, NoteDraft, Project, Tick};
use serde_json::{Value, json};

struct Fixture {
    directory: PathBuf,
    track: Id,
    clip: Id,
    note: Id,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("echode-noop-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let mut session = Session::open(Project::new());
        let Action::AddTrack { id: track, .. } = session
            .perform(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track.add");
        };
        let Action::InsertNote { clip, note, .. } = session
            .perform(Command::InsertNote {
                track,
                draft: NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(705_600),
                },
            })
            .unwrap()
        else {
            panic!("note.insert");
        };
        write_package(&directory, session.project()).unwrap();
        Self {
            directory,
            track,
            clip,
            note,
        }
    }

    fn command(&self, velocity: u8) -> Command {
        Command::SetNoteVelocity {
            track: self.track,
            clip: self.clip,
            note: self.note,
            velocity,
        }
    }

    fn action(&self, velocity: u8) -> Value {
        json!({"type":"note.velocity", "track":self.track, "clip":self.clip,
               "note":self.note, "velocity":velocity})
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn contract_noop_edits_preserve_publication_journal_autosave_and_redo() {
    let fixture = Fixture::new("durability");
    let host = DocumentHost::open(&fixture.directory).unwrap();
    host.perform(fixture.command(50)).unwrap();
    host.undo().unwrap();
    // Listing revisions drains prior saves, so a later difference is attributable
    // to this test's no-op requests, not to an earlier background save.
    let revisions = host.revisions().unwrap();
    let before = host.snapshot();
    let journal = std::fs::read(fixture.directory.join("journal.jsonl")).unwrap();
    host.perform(fixture.command(100)).unwrap();
    let applied = host.apply(vec![fixture.action(100)]).unwrap();
    assert_eq!(applied.len(), 1, "acknowledge every accepted action");
    let cancelled = host
        .apply(vec![fixture.action(30), fixture.action(100)])
        .unwrap();
    assert_eq!(cancelled.len(), 2, "keep result positions for references");
    assert!(
        Arc::ptr_eq(&before, &host.snapshot()),
        "a no-op must not publish"
    );
    assert_eq!(
        host.revisions().unwrap().len(),
        revisions.len(),
        "no-op autosave"
    );
    assert_eq!(
        std::fs::read(fixture.directory.join("journal.jsonl")).unwrap(),
        journal,
        "a no-op must not append a journal line"
    );
    host.redo().expect("no-op requests must preserve redo");
    let expected = host.snapshot();
    drop(host);
    let reopened = DocumentHost::open(&fixture.directory).unwrap();
    assert_eq!(*reopened.snapshot(), *expected);
}

#[test]
fn mixed_edits_keep_one_step_and_failed_edits_keep_redo() {
    let fixture = Fixture::new("mixed");
    let host = DocumentHost::open(&fixture.directory).unwrap();
    let base = host.snapshot();
    let accepted = host
        .apply(vec![
            fixture.action(100),
            fixture.action(50),
            json!({"type":"note.velocity", "track":{"ref":0,"field":"track"},
               "clip":{"ref":1,"field":"clip"}, "note":{"ref":0,"field":"note"},
               "velocity":50}),
        ])
        .unwrap();
    assert_eq!(accepted.len(), 3);
    let edited = host.snapshot();
    host.undo().unwrap();
    assert_eq!(*host.snapshot(), *base);
    assert!(host.undo().is_err(), "mixed edits are one undo step");
    let revisions = host.revisions().unwrap().len();
    let rejected = host.apply(vec![
        fixture.action(100),
        fixture.action(60),
        fixture.action(128),
    ]);
    assert!(rejected.is_err(), "invalid input is not a successful no-op");
    assert_eq!(*host.snapshot(), *base);
    assert_eq!(host.revisions().unwrap().len(), revisions);
    host.redo().unwrap();
    assert_eq!(*host.snapshot(), *edited);
}
