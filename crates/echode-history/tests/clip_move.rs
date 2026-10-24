//! `clip.move` and `clip.duplicate` through the session, the journal, and the package (ADR 0154).

use echode_actions::{ActionError, Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{
    AudioRef, ClipKind, DEFAULT_FADE_FLICKS, Id, ModelError, NoteDraft, Placement, Project,
    TICKS_PER_QUARTER, Tick,
};

const BAR: i64 = TICKS_PER_QUARTER * 4;
const AUDIO: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn clips(project: &Project, track: Id) -> Vec<Placement> {
    project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)
        .unwrap()
        .clips
        .clone()
}

fn perform(session: &mut Session, request: serde_json::Value) {
    let command: Command =
        serde_json::from_value(request).expect("the product accepts the request");
    session.perform(command).unwrap();
    session.project().check_invariants().unwrap();
}

/// A two-note auto clip on `keys`, and three bars of audio on `bed` from bar 2.
fn fixture() -> (Project, Id, Id, Id, Id) {
    let (project, keys) = Project::new().add_track("keys");
    let (project, bed) = project.add_track("bed");
    let draft = |start: i64| NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(start),
        length: Tick::new(TICKS_PER_QUARTER),
    };
    let (project, _) = project.insert_note(keys, draft(0)).unwrap();
    let (project, _) = project
        .insert_note(keys, draft(TICKS_PER_QUARTER * 2))
        .unwrap();
    let audio = Id::generate();
    // At 120 bpm, six seconds of audio is three bars.
    let media = AudioRef::try_new(AUDIO.into(), 48_000, 2, 48_000 * 6).unwrap();
    let project = project
        .place_audio(bed, audio, Id::generate(), Tick::new(BAR * 2), media)
        .unwrap();
    let midi = clips(&project, keys)[0].id;
    (project, keys, bed, midi, audio)
}

#[test]
fn moves_and_copies_survive_history_journal_and_reopen() {
    let directory = std::env::temp_dir().join(format!("echode-clip-move-{}", Id::generate()));
    let (base, keys, bed, midi, audio) = fixture();
    let mut session = Session::open(base.clone());
    let landing = BAR * 3 + TICKS_PER_QUARTER;
    perform(
        &mut session,
        serde_json::json!({ "type": "clip.move", "clip": midi, "track": bed, "start": landing }),
    );
    let moved = session.project().clone();
    assert!(clips(&moved, keys).is_empty());
    let after = clips(&moved, bed);
    let shape: Vec<_> = after
        .iter()
        .map(|clip| {
            (
                clip.start.get(),
                clip.length.get(),
                clip.offset.get(),
                clip.kind,
            )
        })
        .collect();
    let resume = landing + BAR;
    assert_eq!(
        shape,
        [
            (BAR * 2, landing - BAR * 2, 0, ClipKind::Fixed),
            (landing, BAR, 0, ClipKind::Fixed),
            (resume, BAR * 5 - resume, resume - BAR * 2, ClipKind::Fixed),
        ]
    );
    assert_eq!((after[0].id, after[1].id), (audio, midi));
    assert_eq!(
        after[0].content, after[2].content,
        "split pieces share the media content"
    );
    assert_eq!(
        (after[0].fade_out, after[2].fade_in),
        (DEFAULT_FADE_FLICKS, DEFAULT_FADE_FLICKS)
    );

    perform(
        &mut session,
        serde_json::json!({ "type": "clip.duplicate", "clip": midi, "track": keys, "start": BAR }),
    );
    let copied = session.project().clone();
    let copy = clips(&copied, keys)[0].clone();
    assert_ne!(copy.content, after[1].content);
    let note = copied.content(copy.content).unwrap().notes[0].clone();
    session
        .perform(Command::EditNote {
            track: keys,
            clip: copy.id,
            note: note.id,
            pitch: 72,
            start: Tick::new(BAR),
            length: note.length,
        })
        .unwrap();
    assert_eq!(
        session.project().content(after[1].content),
        copied.content(after[1].content),
        "the copy owns its notes"
    );
    session.undo().unwrap();

    session.undo().unwrap();
    assert_eq!(session.project(), &moved);
    session.undo().unwrap();
    assert_eq!(session.project(), &base);
    session.redo().unwrap();
    session.redo().unwrap();
    assert_eq!(session.project(), &copied);
    let journal = encode(session.journal());
    assert_eq!(replay(&base, &decode(&journal).unwrap()).unwrap(), copied);
    let mut actions_only = decode(&journal).unwrap();
    for entry in &mut actions_only {
        entry.changes = None;
    }
    assert_eq!(replay(&base, &actions_only).unwrap(), copied);
    write_package(&directory, &copied).unwrap();
    assert_eq!(read_package(&directory).unwrap(), copied);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn refused_moves_leave_the_project_and_history_unchanged() {
    let (base, keys, bed, midi, _) = fixture();
    let mut session = Session::open(base.clone());
    for (kind, clip, track, start) in [
        ("clip.move", midi, bed, -1),
        ("clip.duplicate", midi, bed, -1),
        ("clip.move", Id::generate(), bed, 0),
        ("clip.duplicate", midi, Id::generate(), 0),
        ("clip.move", midi, keys, i64::MAX),
    ] {
        let command: Command = serde_json::from_value(serde_json::json!({
            "type": kind, "clip": clip, "track": track, "start": start,
        }))
        .unwrap();
        assert!(matches!(
            session.perform(command),
            Err(ActionError::Model(_))
        ));
    }
    assert_eq!(
        session.perform(Command::MoveClip {
            clip: midi,
            track: Id::generate(),
            start: Tick::new(0),
        }),
        Err(ActionError::Model(ModelError::MissingTrack))
    );
    assert_eq!(session.project(), &base);
    assert!(session.journal().is_empty());
}
