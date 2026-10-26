//! Content-relative auto bars through product commands and journal recovery.

use echode_actions::{Command, Session, decode, encode, replay};
use echode_history::{read_package, write_package};
use echode_model::{ClipKind, Id, NoteDraft, Placement, Project, TICKS_PER_QUARTER, Tick};

const BEAT: i64 = TICKS_PER_QUARTER;
const BAR: i64 = BEAT * 4;

fn fixture() -> (Project, Id, Id, Id, Id) {
    let (project, track) = Project::new().add_track("keys");
    let (project, target) = project.add_track("copy");
    let (project, note) = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(BEAT),
                length: Tick::new(BEAT),
            },
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    (project, track, target, clip, note)
}

fn placement(project: &Project, track: Id) -> Placement {
    project
        .tracks()
        .iter()
        .find(|item| item.id == track)
        .unwrap()
        .clips[0]
        .clone()
}

fn perform(session: &mut Session, value: serde_json::Value) {
    session
        .perform(serde_json::from_value::<Command>(value).unwrap())
        .unwrap();
    session.project().check_invariants().unwrap();
}

#[test]
fn contract_shifted_auto_clips_use_content_bars_through_edits_history_and_reopen() {
    let (base, track, target, clip, note) = fixture();
    let directory = std::env::temp_dir().join(format!("echode-content-bars-{}", Id::generate()));
    let origin = BAR * 2 + BEAT / 3;
    let mut session = Session::open(base.clone());
    perform(
        &mut session,
        serde_json::json!({"type":"clip.move", "clip":clip, "track":track, "start":origin}),
    );
    let shifted = session.project().clone();
    assert_eq!(
        placement(&shifted, track).kind,
        ClipKind::Auto,
        "a sub-bar move must preserve the automatic content-bar window"
    );
    perform(
        &mut session,
        serde_json::json!({"type":"clip.duplicate", "clip":clip, "track":target, "start":BEAT / 2}),
    );
    let copied = placement(session.project(), target);
    assert_eq!(copied.kind, ClipKind::Auto);
    assert_ne!(copied.content, placement(&shifted, track).content);

    // Extend both ways across a content bar, not the timeline's bar lines.
    perform(
        &mut session,
        serde_json::json!({"type":"note.edit", "track":track, "clip":clip, "note":note,
            "pitch":65, "start":origin - 1, "length":BAR + 2}),
    );
    let grown = placement(session.project(), track);
    assert_eq!(
        (grown.start.get(), grown.offset.get(), grown.length.get()),
        (origin - BAR, -BAR, BAR * 3)
    );
    let stored = &session.project().content(grown.content).unwrap().notes[0];
    assert_eq!(stored.id, note);
    assert_eq!(stored.velocity, base.contents()[0].notes[0].velocity);
    assert_eq!(grown.sounding_start(stored), Some(Tick::new(origin - 1)));
    assert_eq!(placement(session.project(), target), copied);
    let before_shortening = session.project().clone();
    perform(
        &mut session,
        serde_json::json!({"type":"note.edit", "track":track, "clip":clip, "note":note,
            "pitch":65, "start":origin + BEAT, "length":BEAT}),
    );
    assert_eq!(placement(session.project(), track).start.get(), origin);
    assert_eq!(placement(session.project(), track).length.get(), BAR);
    session.undo().unwrap();
    assert_eq!(session.project(), &before_shortening);
    session.redo().unwrap();

    perform(
        &mut session,
        serde_json::json!({"type":"note.insert", "track":track, "pitch":67, "velocity":90,
            "start":origin + BAR, "length":BEAT}),
    );
    let extended = placement(session.project(), track);
    assert_eq!(
        (extended.start.get(), extended.length.get()),
        (origin, BAR * 2)
    );
    let inserted = session.project().content(extended.content).unwrap().notes[1].id;
    perform(
        &mut session,
        serde_json::json!({"type":"note.delete", "track":track, "note":inserted}),
    );
    assert_eq!(placement(session.project(), track).length.get(), BAR);

    let final_project = session.project().clone();
    let mut journal = decode(&encode(session.journal())).unwrap();
    assert_eq!(replay(&base, &journal).unwrap(), final_project);
    for entry in &mut journal {
        entry.changes = None;
    }
    assert_eq!(replay(&base, &journal).unwrap(), final_project);
    write_package(&directory, &final_project).unwrap();
    assert_eq!(read_package(&directory).unwrap(), final_project);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn contract_unmarked_clip_journals_keep_the_legacy_fixed_kind() {
    for kind in ["clip.move", "clip.duplicate"] {
        let (base, track, target, clip, _) = fixture();
        let mut session = Session::open(base.clone());
        perform(
            &mut session,
            serde_json::json!({"type":kind, "clip":clip, "track":target, "start":BEAT / 3}),
        );
        let marked = session.project().clone();
        assert_eq!(placement(&marked, target).kind, ClipKind::Auto);
        let mut old: serde_json::Value = serde_json::from_str(&encode(session.journal())).unwrap();
        old.as_object_mut().unwrap().remove("changes");
        for action in old["actions"].as_array_mut().unwrap() {
            action.as_object_mut().unwrap().remove("content_bars");
        }
        let old = decode(&serde_json::to_string(&old).unwrap()).unwrap();
        let restored = replay(&base, &old).unwrap();
        restored.check_invariants().unwrap();
        assert_eq!(
            placement(&restored, target).kind,
            ClipKind::Fixed,
            "an unmarked journal must replay the original sub-bar move rule"
        );
        if kind == "clip.duplicate" {
            assert_eq!(placement(&restored, track), placement(&base, track));
        }
        let mut expected = marked;
        let moved = placement(&expected, target);
        expected = expected
            .resize_clip(target, moved.id, moved.start, moved.length)
            .unwrap();
        assert_eq!(restored, expected);
    }
}
