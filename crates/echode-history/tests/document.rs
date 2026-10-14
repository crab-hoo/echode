use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use echode_actions::{Action, Command, JournalLine, JournalOp, Session};
use echode_history::{
    AUTOSAVE_REF, Autosave, append_journal, read_package, recover, write_package,
};
use echode_model::{
    ClipKind, Content, DEFAULT_FADE_FLICKS, Device, Id, Lane, Meter, MixerIds, Note, NoteDraft,
    Placement, Project, TICKS_PER_QUARTER, Tempo, TempoMap, Tick, Track, initial_order,
    midi7_to_velocity,
};

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
    let track_text = std::str::from_utf8(&before).unwrap();
    assert!(track_text.contains("pi\\\"ano"));
    assert!(track_text.contains("\"content\":"));
    assert!(track_text.contains("\"lane\":\"timeline\""));
    let content_id = project.contents()[0].id;
    assert!(
        directory
            .join("contents")
            .join(format!("{content_id}.json"))
            .is_file()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_fade_past_the_frame_limit_is_rejected_on_load() {
    let directory = scratch();
    let project = with_note("piano");
    write_package(&directory, &project).unwrap();
    let path = directory
        .join("tracks")
        .join(format!("{}.json", project.tracks()[0].id));
    let rate = i128::from(768_000);
    let frames = i128::from(u32::MAX) + 1;
    let too_wide = i64::try_from((frames * 705_600_000 + rate - 1) / rate).unwrap();
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"fade_in\":3528000", &format!("\"fade_in\":{too_wide}"));
    std::fs::write(&path, text).unwrap();
    let error = read_package(&directory).unwrap_err();
    assert!(error.to_string().contains("fade"), "{error}");
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
    let entry = session.journal().last().unwrap();
    append_journal(
        &directory,
        &JournalLine {
            seq: 1,
            op: JournalOp::Edit,
            actions: entry.actions.clone(),
            changes: entry.changes.clone().unwrap_or_default(),
        },
    )
    .unwrap();
    assert_eq!(recover(&directory).unwrap(), *session.project());

    let autosave = Autosave::create(&directory).unwrap();
    let first = autosave
        .save(Arc::new(session.project().clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(autosave.load(first).unwrap(), *session.project());
    drop(autosave);
    assert_eq!(recover(&directory).unwrap(), *session.project());

    let autosave = Autosave::create(&directory).unwrap();
    let second = autosave
        .save(Arc::new(saved.clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
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
fn a_torn_journal_tail_keeps_the_finished_lines() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    append_line(&directory, &session, 1);
    session
        .perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap();
    append_line(&directory, &session, 2);
    let path = directory.join("journal.jsonl");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 6]).unwrap();
    let project = recover(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(project.tracks().len(), 1);
    assert_eq!(project.tracks()[0].name, "piano");
}

#[test]
fn an_edit_after_the_commit_is_replayed() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    append_line(&directory, &session, 1);
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(Arc::new(session.project().clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
    session
        .perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap();
    append_line(&directory, &session, 2);
    let project = recover(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(
        project
            .tracks()
            .iter()
            .map(|track| track.name.as_str())
            .collect::<Vec<_>>(),
        vec!["piano", "drums"]
    );
}

#[test]
fn an_undo_after_compaction_undoes_the_committed_edit() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    let changes = session.journal().last().unwrap().changes.clone().unwrap();
    append_line(&directory, &session, 1);
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(Arc::new(session.project().clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
    echode_history::compact_journal(&directory, 1).unwrap();
    append_journal(
        &directory,
        &JournalLine {
            seq: 2,
            op: JournalOp::Undo,
            actions: Vec::new(),
            changes,
        },
    )
    .unwrap();
    let project = recover(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert!(project.tracks().is_empty());
}

#[test]
fn a_corrupt_tip_returns_the_parent_without_the_gapped_lines() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    let autosave = Autosave::create(&directory).unwrap();
    let parent = autosave
        .save(Arc::new(session.project().clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    session
        .perform(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap();
    let tip = autosave
        .save(Arc::new(session.project().clone()), 2)
        .unwrap()
        .wait()
        .unwrap();
    drop(autosave);
    echode_history::compact_journal(&directory, 2).unwrap();
    let hex = tip.to_string();
    let object = directory
        .join(".git/objects")
        .join(&hex[..2])
        .join(&hex[2..]);
    std::fs::remove_file(&object).unwrap();
    let project = recover(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(project.tracks().len(), 1);
    assert_eq!(project.tracks()[0].name, "piano");
    assert_ne!(parent, tip);
}

#[test]
fn a_pending_save_does_not_replay_onto_the_worktree() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap();
    append_line(&directory, &session, 1);
    echode_history::mark_save_pending(&directory).unwrap();
    let error = recover(&directory).unwrap_err().to_string();
    let _ = std::fs::remove_dir_all(directory);
    assert!(error.contains("unfinished"), "{error}");
}

#[test]
fn a_base_commit_keeps_the_journal_when_the_next_save_is_pending() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = echode_history::DocumentHost::open(&directory).unwrap();
    drop(host);
    echode_history::mark_save_pending(&directory).unwrap();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "kept".into(),
        })
        .unwrap();
    append_line(&directory, &session, 1);
    let project = recover(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(project.tracks().len(), 1);
    assert_eq!(project.tracks()[0].name, "kept");
}

fn append_line(directory: &std::path::Path, session: &Session, seq: u64) {
    let entry = session.journal().last().unwrap();
    append_journal(
        directory,
        &JournalLine {
            seq,
            op: JournalOp::Edit,
            actions: entry.actions.clone(),
            changes: entry.changes.clone().unwrap(),
        },
    )
    .unwrap();
}

#[test]
fn a_fader_at_zero_roundtrips_through_the_package() {
    let mut session = Session::open(Project::new());
    let Action::AddTrack { gain, .. } = session
        .perform(Command::AddTrack {
            name: "piano".into(),
        })
        .unwrap()
    else {
        panic!("add track");
    };
    session
        .perform(Command::SetParameter {
            parameter: gain,
            value: 0.0,
        })
        .unwrap();
    let directory = scratch();
    write_package(&directory, session.project()).unwrap();
    let loaded = read_package(&directory).unwrap();
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(loaded.parameter(gain).unwrap().value, 0.0);
    assert_eq!(&loaded, session.project());
}

#[test]
fn a_note_reserves_release_articulation_and_expressions() {
    let directory = scratch();
    let project = with_note("piano");
    write_package(&directory, &project).unwrap();
    let path = directory
        .join("contents")
        .join(format!("{}.json", project.contents()[0].id));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"release\":0"));
    assert!(text.contains("\"articulation\":null"));
    assert!(text.contains("\"expressions\":[]"));
    assert!(text.contains("\"velocity\":51603"));
    let with_expression = text.replace(
        "\"expressions\":[]",
        "\"expressions\":[{\"note\":\"00000000-0000-0000-0000-000000000001\",\"kind\":\"pitch\",\"points\":[{\"at\":0,\"value\":0}]}]",
    );
    std::fs::write(&path, with_expression).unwrap();
    let error = read_package(&directory).unwrap_err();
    assert!(error.to_string().contains("expressions"), "{error}");
    let with_articulation = text.replace(
        "\"articulation\":null",
        "\"articulation\":\"00000000-0000-0000-0000-000000000001\"",
    );
    std::fs::write(&path, with_articulation).unwrap();
    let error = read_package(&directory).unwrap_err();
    assert!(error.to_string().contains("articulation"), "{error}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_device_state_is_null_until_plugins_can_hold_one() {
    let directory = scratch();
    let project = with_note("piano");
    write_package(&directory, &project).unwrap();
    let path = directory
        .join("tracks")
        .join(format!("{}.json", project.tracks()[0].id));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"state\":null"), "{text}");
    assert_eq!(read_package(&directory).unwrap(), project);
    std::fs::write(&path, text.replace("\"state\":null", "\"state\":\"abc\"")).unwrap();
    let error = read_package(&directory).unwrap_err().to_string();
    assert!(error.contains("mixer state must be null"), "{error}");
    let hash = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    std::fs::write(
        &path,
        text.replace("\"state\":null", &format!("\"state\":\"{hash}\"")),
    )
    .unwrap();
    let error = read_package(&directory).unwrap_err().to_string();
    assert!(error.contains("mixer state must be null"), "{error}");
    let device = Id::from_raw(90);
    let project = project
        .add_clap_device(project.tracks()[0].id, device, "dev.echode.test-state")
        .unwrap()
        .set_device_state(device, Some(hash.to_string()))
        .unwrap();
    write_package(&directory, &project).unwrap();
    let clap_path = directory
        .join("tracks")
        .join(format!("{}.json", project.tracks()[0].id));
    let body = std::fs::read_to_string(&clap_path).unwrap();
    std::fs::write(
        &clap_path,
        body.replacen(&format!("\"state\":\"{hash}\""), "\"state\":\"abc\"", 1),
    )
    .unwrap();
    let bad = read_package(&directory).unwrap_err().to_string();
    assert!(bad.contains("64"), "{bad}");
    write_package(&directory, &project).unwrap();
    let missing = read_package(&directory).unwrap_err().to_string();
    assert!(missing.contains(hash), "{missing}");
    assert_eq!(echode_history::write_object(&directory, &[]).unwrap(), hash);
    assert_eq!(read_package(&directory).unwrap(), project);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_package_repo_ignores_the_object_store() {
    let directory = scratch();
    let autosave = Autosave::create(&directory).unwrap();
    let text = std::fs::read_to_string(directory.join(".gitignore")).unwrap();
    drop(autosave);
    let _ = std::fs::remove_dir_all(directory);
    assert!(text.contains("/media/objects/\n"), "{text}");
    assert!(text.contains("/cache/\n"), "{text}");
}

#[test]
fn a_bad_schema_is_rejected() {
    let directory = scratch();
    write_package(&directory, &with_note("piano")).unwrap();
    std::fs::write(directory.join("project.json"), "{\"schema\": 4}\n").unwrap();
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
            .save(Arc::new(session.project().clone()), 0)
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
            .save(Arc::new(session.project().clone()), 0)
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(autosave.load(commit).unwrap(), *session.project());
        drop(autosave);
        let _ = std::fs::remove_dir_all(directory);
    }
}

fn schema_1_project() -> Project {
    let content_id = Id::from_raw(1);
    let note_id = Id::from_raw(2);
    let track_id = Id::from_raw(3);
    let clip_id = Id::from_raw(4);
    let tempo = Tempo::quarters_per_minute(120).unwrap();
    Project::try_assemble(
        Meter::four_four(),
        TempoMap::new(tempo),
        vec![Content {
            id: content_id,
            audio: None,
            takes: Vec::new(),
            notes: vec![Note {
                id: note_id,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER),
                pitch: 60,
                velocity: midi7_to_velocity(100),
                release: 0,
                articulation: None,
            }],
            expressions: Vec::new(),
        }],
        vec![Track {
            id: track_id,
            name: "piano".into(),
            parent: None,
            order: initial_order(),
            devices: vec![Device::unity(MixerIds::from_base(0x10))],
            clips: vec![Placement {
                id: clip_id,
                content: content_id,
                lane: Lane::Timeline,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER),
                offset: Tick::new(0),
                kind: ClipKind::Fixed,
                gain: 1.0,
                fade_in: DEFAULT_FADE_FLICKS,
                fade_out: DEFAULT_FADE_FLICKS,
            }],
        }],
    )
    .unwrap()
}

#[test]
fn the_schema_1_fixture_loads_as_the_current_model() {
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schema-1");
    let loaded = read_package(&fixture).unwrap();
    assert_eq!(loaded, schema_1_project());
}

#[test]
fn the_package_text_matches_the_snapshot() {
    let directory = scratch();
    let project = schema_1_project();
    write_package(&directory, &project).unwrap();
    let project_json = std::fs::read_to_string(directory.join("project.json")).unwrap();
    let track_json = std::fs::read_to_string(
        directory
            .join("tracks")
            .join(format!("{}.json", project.tracks()[0].id)),
    )
    .unwrap();
    let content_json = std::fs::read_to_string(
        directory
            .join("contents")
            .join(format!("{}.json", project.contents()[0].id)),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(directory);
    insta::assert_snapshot!("project_json", project_json);
    insta::assert_snapshot!("track_json", track_json);
    insta::assert_snapshot!("content_json", content_json);
}

#[test]
fn a_schema_other_than_the_current_one_is_rejected() {
    let project = schema_1_project();
    let files = ["project", "track", "content"];
    for kind in files {
        let directory = scratch();
        write_package(&directory, &project).unwrap();
        let path = match kind {
            "project" => directory.join("project.json"),
            "track" => directory
                .join("tracks")
                .join(format!("{}.json", project.tracks()[0].id)),
            "content" => directory
                .join("contents")
                .join(format!("{}.json", project.contents()[0].id)),
            _ => unreachable!(),
        };
        let original = std::fs::read_to_string(&path).unwrap();
        for schema in ["4", "0", "2.0"] {
            let text = original.replacen("\"schema\":3", &format!("\"schema\":{schema}"), 1);
            std::fs::write(&path, &text).unwrap();
            let error = read_package(&directory).unwrap_err().to_string();
            assert!(
                error.contains("schema") && error.contains(kind),
                "{kind} {schema}: {error}"
            );
        }
        let missing = original.replacen("\"schema\":3,", "", 1);
        std::fs::write(&path, missing).unwrap();
        let error = read_package(&directory).unwrap_err().to_string();
        assert!(
            error.contains("missing") && error.contains(kind),
            "{kind}: {error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_removed_track_file_is_deleted_on_the_next_save() {
    let directory = scratch();
    let (project, kept) = Project::new().add_track("kept");
    let (project, dropped) = project.add_track("dropped");
    let autosave = Autosave::create(&directory).unwrap();
    autosave
        .save(Arc::new(project.clone()), 0)
        .unwrap()
        .wait()
        .unwrap();
    let tracks = project
        .tracks()
        .iter()
        .filter(|track| track.id == kept)
        .map(|track| (**track).clone())
        .collect();
    let next = Project::try_assemble(project.meter(), project.tempo().clone(), Vec::new(), tracks)
        .unwrap();
    autosave
        .save(Arc::new(next.clone()), 1)
        .unwrap()
        .wait()
        .unwrap();
    assert!(
        directory
            .join("tracks")
            .join(format!("{kept}.json"))
            .is_file()
    );
    assert!(
        !directory
            .join("tracks")
            .join(format!("{dropped}.json"))
            .exists()
    );
    drop(autosave);
    assert_eq!(read_package(&directory).unwrap(), next);
    let _ = std::fs::remove_dir_all(directory);
}
