//! Full-resolution note copies through product commands, recovery, and storage.
use echode_actions::{Action, Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{Id, NoteDraft, Project, Tick};

#[test]
fn contract_note_copies_preserve_sources_attributes_and_history() {
    let directory = std::env::temp_dir().join(format!("echode-note-copy-{}", Id::generate()));
    let (project, track) = Project::new().add_track("keys");
    let (project, source) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(705_600),
                length: Tick::new(705_600),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let content = project.tracks()[0].clips[0].content;
    write_package(&directory, &project).unwrap();
    // Exercise valid imported 16-bit values that cannot round-trip through MIDI 1.
    let path = directory.join("contents").join(format!("{content}.json"));
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["notes"][0]["velocity"] = 12_345.into();
    json["notes"][0]["release"] = 54_321.into();
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let project = read_package(&directory).unwrap();
    let link = Id::generate();
    let base = project
        .link_clip(clip, track, Tick::new(8 * 705_600), link)
        .unwrap();
    let original = base.content(content).unwrap().notes[0].clone();
    let copied = Id::generate();
    let mut session = Session::open(base.clone());
    session
        .perform(Command::DuplicateNote {
            track,
            clip: link,
            note: source,
            copy: Some(copied),
            pitch: 64,
            start: Tick::new(10 * 705_600 + 123),
        })
        .unwrap();
    session.project().check_invariants().unwrap();
    let notes = &session.project().content(content).unwrap().notes;
    assert_eq!(
        notes.iter().find(|note| note.id == source),
        Some(&original),
        "copying must not edit the source"
    );
    let mut expected = original.clone();
    expected.id = copied;
    expected.pitch = 64;
    expected.start = Tick::new(2 * 705_600 + 123);
    assert_eq!(
        notes.iter().find(|note| note.id == copied),
        Some(&expected),
        "copies must preserve full-resolution attack and release velocities"
    );
    let edited = session.project().clone();
    session.undo().unwrap();
    assert_eq!(session.project(), &base);
    session.redo().unwrap();
    assert_eq!(session.project(), &edited);
    for (bad_track, bad_clip, bad_note, copy, pitch, start) in [
        (Id::generate(), link, source, Id::generate(), 64, 0),
        (track, Id::generate(), source, Id::generate(), 64, 0),
        (track, link, Id::generate(), Id::generate(), 64, 0),
        (track, link, source, source, 64, 0),
        (track, link, source, track, 64, 0),
        (track, link, source, Id::generate(), 128, 0),
        (track, link, source, Id::generate(), 64, i64::MAX),
    ] {
        assert!(
            session
                .perform(Command::DuplicateNote {
                    track: bad_track,
                    clip: bad_clip,
                    note: bad_note,
                    copy: Some(copy),
                    pitch,
                    start: Tick::new(start),
                })
                .is_err()
        );
        assert_eq!(
            session.project(),
            &edited,
            "invalid copies must leave the model unchanged"
        );
    }
    // Omitting the optional copy id must still journal a stable generated id.
    let action = session
        .perform(
            serde_json::from_value(serde_json::json!({
                "type": "note.duplicate", "track": track, "clip": link, "note": source,
                "pitch": 60, "start": 9 * 705_600,
            }))
            .unwrap(),
        )
        .unwrap();
    let Action::DuplicateNote { copy, .. } = action else {
        panic!("copy action")
    };
    assert_ne!(copy, source);
    assert_eq!(session.project().content(content).unwrap().notes.len(), 3);
    let mut journal = decode(&encode(session.journal())).unwrap();
    assert_eq!(replay(&base, &journal).unwrap(), *session.project());
    for entry in &mut journal {
        entry.changes = None;
    }
    assert_eq!(
        replay(&base, &journal).unwrap(),
        *session.project(),
        "action-only recovery must preserve copied ids and attributes"
    );
    write_package(&directory, session.project()).unwrap();
    assert_eq!(read_package(&directory).unwrap(), *session.project());
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(base.content(content).unwrap().notes, vec![original].into());
}
