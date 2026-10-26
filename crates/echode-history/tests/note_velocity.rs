//! Attack velocity through product commands, Undo, recovery, and package storage.
use echode_actions::{Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{Id, Note, NoteDraft, Project, Tick, midi7_to_velocity};

fn stored(project: &Project, id: Id) -> &Note {
    project.contents()[0]
        .notes
        .iter()
        .find(|item| item.id == id)
        .unwrap()
}

#[test]
fn contract_velocity_preserves_shared_content_windows_and_round_trips_history() {
    let (project, track) = Project::new().add_track("keys");
    let (project, linked_track) = project.add_track("linked");
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(6).unwrap(),
            },
        )
        .unwrap();
    let (project, other) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 42,
                start: Tick::new(0),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(
            track,
            clip,
            Tick::quarters(1).unwrap(),
            Tick::quarters(2).unwrap(),
        )
        .unwrap();
    let link = Id::generate();
    let base = project
        .link_clip(clip, linked_track, Tick::quarters(8).unwrap(), link)
        .unwrap();
    let original = stored(&base, note).clone();
    let untouched = stored(&base, other).clone();
    let mut session = Session::open(base.clone());
    for velocity in [0, 1, 64, 127] {
        let before = session.project().clone();
        session
            .perform(Command::SetNoteVelocity {
                track: linked_track,
                clip: link,
                note,
                velocity,
            })
            .unwrap();
        session.project().check_invariants().unwrap();
        assert_eq!(
            stored(session.project(), note).velocity,
            midi7_to_velocity(velocity),
            "velocity must reach shared content"
        );
        let mut expected = original.clone();
        expected.velocity = midi7_to_velocity(velocity);
        assert_eq!(stored(session.project(), note), &expected);
        assert_eq!(
            session.project().tracks(),
            base.tracks(),
            "velocity must not untrim or refit any window"
        );
        assert_eq!(stored(session.project(), other), &untouched);
        let edited = session.project().clone();
        session.undo().unwrap();
        assert_eq!(session.project(), &before);
        session.redo().unwrap();
        assert_eq!(session.project(), &edited);
    }
    for (track, clip, note, velocity) in [
        (track, clip, note, 128),
        (Id::generate(), clip, note, 50),
        (track, link, note, 50),
        (track, clip, Id::generate(), 50),
    ] {
        let before = session.project().clone();
        assert!(
            session
                .perform(Command::SetNoteVelocity {
                    track,
                    clip,
                    note,
                    velocity
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }
    let mut journal = decode(&encode(session.journal())).unwrap();
    assert_eq!(replay(&base, &journal).unwrap(), *session.project());
    for entry in &mut journal {
        entry.changes = None;
    }
    assert_eq!(replay(&base, &journal).unwrap(), *session.project());
    let directory = std::env::temp_dir().join(format!("echode-velocity-{}", Id::generate()));
    write_package(&directory, session.project()).unwrap();
    assert_eq!(read_package(&directory).unwrap(), *session.project());
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(stored(&base, note), &original);
}
