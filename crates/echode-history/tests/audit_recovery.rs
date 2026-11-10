//! Reproductions from the 2026-10-27 adversarial audit (ADV-01, ADV-04).
//!
//! Both findings run as normal regressions.
//! See docs/reviews/2026-10-27-adversarial-audit.md.

use std::path::PathBuf;

use echode_actions::{Command, JournalLine, JournalOp, Session, encode_line};
use echode_history::{DocumentHost, recover, write_package};
use echode_model::{Change, ChangeSet, Id, NoteDraft, ObjectKey, Project, Property, Tick, Value};

const QUARTER: i64 = 705_600;
/// The audio clips are one second long: two beats at the default 120 BPM.
const CLIP_TICKS: i64 = 2 * QUARTER;

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir()
        .join(format!(
            "echode-audit-recovery-{}-{name}",
            std::process::id()
        ))
        .join("song.echode");
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    directory
}

fn place(host_or_session: &mut dyn FnMut(Command), track: Id) {
    for start in [0, 8 * QUARTER] {
        host_or_session(Command::PlaceAudio {
            track,
            start: Tick::new(start),
            hash: "ab".repeat(32),
            sample_rate: 48_000,
            channels: 2,
            frames: 48_000,
        });
    }
}

/// A valid move of the second clip to beat 4.
fn move_line(project: &Project, seq: u64) -> JournalLine {
    let track = project.tracks()[0].id;
    let clip = project.tracks()[0].clips[1].id;
    let mut session = Session::open(project.clone());
    session
        .perform(Command::MoveClip {
            clip,
            track,
            start: Tick::new(4 * QUARTER),
        })
        .unwrap();
    let entry = session.journal().last().unwrap().clone();
    JournalLine {
        seq,
        op: JournalOp::Edit,
        actions: entry.actions,
        changes: entry.changes.unwrap(),
    }
}

fn damaged_move(project: &Project, seq: u64) -> String {
    let line = encode_line(&move_line(project, seq));
    let intended = format!("\"after\":{{\"type\":\"tick\",\"tick\":{}}}", 4 * QUARTER);
    let damaged = format!(
        "\"after\":{{\"type\":\"tick\",\"tick\":{}}}",
        CLIP_TICKS / 2
    );
    assert!(line.contains(&intended), "{line}");
    format!("{}\n", line.replace(&intended, &damaged))
}

fn autosave_tip(directory: &std::path::Path) -> Option<gix::ObjectId> {
    let repo = gix::open(directory).ok()?;
    Some(
        repo.try_find_reference("refs/echode/autosave/main")
            .ok()??
            .id()
            .detach(),
    )
}

fn assert_refused(directory: &std::path::Path, seq: u64) -> echode_model::InvariantError {
    let journal = std::fs::read(directory.join("journal.jsonl")).unwrap();
    let project = echode_history::read_package(directory).unwrap();
    let tip = autosave_tip(directory);
    let expected = format!("journal seq {seq} violates model invariants:");
    let error = recover(directory).expect_err("recovery must refuse an invalid journal step");
    assert!(error.to_string().contains(&expected), "{error}");
    assert!(matches!(
        &error,
        echode_history::PackageError::Journal(echode_history::JournalError::Invariants { seq: actual, error: cause })
            if *actual == seq && std::error::Error::source(&error).unwrap().source().unwrap().is::<echode_model::InvariantError>()
                && error.to_string() == format!("journal seq {seq} violates model invariants: {cause}")
    ));
    // A failed open releases its lock and never reaches autosave or compaction.
    for _ in 0..2 {
        let error = DocumentHost::open(directory)
            .err()
            .expect("the product open path must refuse an invalid journal step");
        assert!(error.to_string().contains(&expected), "{error}");
        assert_eq!(
            std::fs::read(directory.join("journal.jsonl")).unwrap(),
            journal
        );
        assert_eq!(echode_history::read_package(directory).unwrap(), project);
        assert_eq!(autosave_tip(directory), tip);
        assert!(!directory.join("save.pending").exists());
    }
    std::error::Error::source(&error)
        .unwrap()
        .source()
        .unwrap()
        .downcast_ref::<echode_model::InvariantError>()
        .unwrap()
        .clone()
}

#[test]
fn audit_adv01_recovery_refuses_a_journal_line_that_breaks_invariants() {
    let directory = scratch("refuse");
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = session.project().tracks()[0].id;
    place(
        &mut |command| {
            session.perform(command).unwrap();
        },
        track,
    );
    let base = session.project().clone();
    write_package(&directory, &base).unwrap();
    std::fs::write(directory.join("journal.jsonl"), damaged_move(&base, 1)).unwrap();

    assert_eq!(
        assert_refused(&directory, 1),
        echode_model::InvariantError::ClipOverlap {
            clip: base.tracks()[0].clips[1].id,
            track,
            track_name: "a".into(),
        }
    );
    assert!(autosave_tip(&directory).is_none());
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

#[test]
fn audit_adv01_an_edit_confirmed_after_recovery_survives_reopening() {
    let directory = scratch("survive");
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = host.snapshot().tracks()[0].id;
    place(
        &mut |command| {
            host.perform(command).unwrap();
        },
        track,
    );
    drop(host);
    // The document already has an autosave tip; an interrupted session left one line.
    let (project, seq) = {
        let host = DocumentHost::open(&directory).unwrap();
        let (_, seq) = host.recovered_label().unwrap();
        ((*host.snapshot()).clone(), seq)
    };
    std::fs::write(
        directory.join("journal.jsonl"),
        damaged_move(&project, seq + 1),
    )
    .unwrap();

    assert_refused(&directory, seq + 1);

    // An explicit repair restores the original valid record. This is test-only;
    // opening a damaged journal must never silently discard or rewrite it.
    std::fs::write(
        directory.join("journal.jsonl"),
        format!("{}\n", encode_line(&move_line(&project, seq + 1))),
    )
    .unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "confirmed".into(),
    })
    .unwrap();
    drop(host);
    let reopened = DocumentHost::open(&directory).unwrap();
    let names: Vec<String> = reopened
        .snapshot()
        .tracks()
        .iter()
        .map(|track| track.name.clone())
        .collect();
    assert!(
        names.iter().any(|name| name == "confirmed"),
        "an edit reported as saved was lost on reopen: {names:?}"
    );
    drop(reopened);
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

#[test]
fn journal_rejects_an_invalid_intermediate_step_for_edit_undo_and_redo() {
    let directory = scratch("intermediate");
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = session.project().tracks()[0].id;
    place(
        &mut |command| {
            session.perform(command).unwrap();
        },
        track,
    );
    let base = session.project().clone();
    write_package(&directory, &base).unwrap();
    session
        .perform(Command::AddTrack {
            name: "valid prefix".into(),
        })
        .unwrap();
    let prefix = session.journal().last().unwrap();
    let prefix = JournalLine {
        seq: 1,
        op: JournalOp::Edit,
        actions: prefix.actions.clone(),
        changes: prefix.changes.clone().unwrap(),
    };
    let damaged = echode_actions::decode_line(&damaged_move(session.project(), 2)).unwrap();
    for op in [JournalOp::Edit, JournalOp::Undo, JournalOp::Redo] {
        let mut invalid = damaged.clone();
        invalid.op = op;
        if op == JournalOp::Undo {
            invalid.changes = invalid.changes.invert();
        }
        let repair = JournalLine {
            seq: 3,
            op: JournalOp::Undo,
            actions: Vec::new(),
            changes: damaged.changes.clone(),
        };
        // The final state is valid, but the complete second step is not.
        // Checking only the final replay result would hide the corruption.
        let intermediate = session.project().apply_changeset(&damaged.changes).unwrap();
        assert!(intermediate.check_invariants().is_err());
        assert_eq!(
            intermediate
                .apply_changeset(&repair.changes.invert())
                .unwrap(),
            *session.project(),
        );
        std::fs::write(
            directory.join("journal.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                encode_line(&prefix),
                encode_line(&invalid),
                encode_line(&repair)
            ),
        )
        .unwrap();
        assert_refused(&directory, 2);
    }
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

#[test]
fn journal_rejects_out_of_range_and_missing_mixer_parameters() {
    let directory = scratch("mixer");
    let (base, _) = Project::new().add_track("a");
    write_package(&directory, &base).unwrap();
    let mixer = &base.tracks()[0].devices[0];
    let parameter = &mixer.parameters[0];
    let mut missing = ChangeSet::new();
    for item in &mixer.parameters {
        missing.push(Change::Delete {
            kind: echode_model::ObjectKind::Parameter,
            id: ObjectKey::Id(item.id),
            fields: Vec::new(),
        });
    }
    missing.push(Change::Update {
        id: ObjectKey::Id(mixer.id),
        property: Property::ParamOrder,
        before: Value::Ids(mixer.parameters.iter().map(|item| item.id).collect()),
        after: Value::Ids(Vec::new()),
    });
    let mut out_of_range = ChangeSet::new();
    out_of_range.push(Change::Update {
        id: ObjectKey::Id(parameter.id),
        property: Property::Value,
        before: Value::Bits(parameter.value.to_bits()),
        after: Value::Bits((parameter.max + 1.0).to_bits()),
    });
    for changes in [missing, out_of_range] {
        assert!(
            base.apply_changeset(&changes)
                .unwrap()
                .check_invariants()
                .is_err()
        );
        let line = JournalLine {
            seq: 1,
            op: JournalOp::Edit,
            actions: Vec::new(),
            changes,
        };
        std::fs::write(
            directory.join("journal.jsonl"),
            format!("{}\n", encode_line(&line)),
        )
        .unwrap();
        assert_refused(&directory, 1);
    }
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

#[test]
fn audit_adv04_an_edit_that_changes_nothing_adds_no_undo_step() {
    let directory = scratch("no-op");
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = host.snapshot().tracks()[0].id;
    host.perform(Command::InsertNote {
        track,
        draft: NoteDraft {
            pitch: 60,
            velocity: 100,
            start: Tick::new(0),
            length: Tick::new(QUARTER),
        },
    })
    .unwrap();
    let project = host.snapshot();
    let placement = &project.tracks()[0].clips[0];
    let note = project.content(placement.content).unwrap().notes[0].id;
    // The same velocity, as an API or MCP client might resend it.
    host.perform(Command::SetNoteVelocity {
        track,
        clip: placement.id,
        note,
        velocity: 100,
    })
    .unwrap();
    host.undo().unwrap();
    let notes: usize = host
        .snapshot()
        .contents()
        .iter()
        .map(|content| content.notes.len())
        .sum();
    assert_eq!(
        notes, 0,
        "the first undo removed an empty step, not the note"
    );
    drop(host);
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}
