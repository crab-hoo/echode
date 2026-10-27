//! Reproductions from the 2026-10-27 adversarial audit (ADV-01, ADV-04).
//!
//! Each test asserts the intended behavior and is ignored until fixed, so
//! `make known-bugs` shows it failing. See docs/reviews/2026-10-27-adversarial-audit.md.

use std::path::PathBuf;

use echode_actions::{Command, JournalLine, JournalOp, Session, encode_line};
use echode_history::{DocumentHost, recover, write_package};
use echode_model::{Id, NoteDraft, Project, Tick};

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

/// A journal line that moves the second clip to beat 4, then one stored tick
/// changed so the clip lands on beat 1, inside the first clip.
fn damaged_move(project: &Project, seq: u64) -> String {
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
    let line = encode_line(&JournalLine {
        seq,
        op: JournalOp::Edit,
        actions: entry.actions,
        changes: entry.changes.unwrap(),
    });
    let intended = format!("\"after\":{{\"type\":\"tick\",\"tick\":{}}}", 4 * QUARTER);
    let damaged = format!(
        "\"after\":{{\"type\":\"tick\",\"tick\":{}}}",
        CLIP_TICKS / 2
    );
    assert!(line.contains(&intended), "{line}");
    format!("{}\n", line.replace(&intended, &damaged))
}

#[test]
#[ignore = "ADV-01: recovery applies journal changesets without checking invariants"]
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

    if let Ok(project) = recover(&directory) {
        assert_eq!(
            project.check_invariants(),
            Ok(()),
            "recover returned a project that breaks the model rules"
        );
    }
    if let Ok(host) = DocumentHost::open(&directory) {
        assert_eq!(
            host.snapshot().check_invariants(),
            Ok(()),
            "the product open path accepted a project that breaks the model rules"
        );
    }
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
}

#[test]
#[ignore = "ADV-01: an invalid recovered tip is skipped on reopen, losing later edits"]
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

    // Either the open refuses the damaged line, or what it confirms must last.
    let Ok(host) = DocumentHost::open(&directory) else {
        return;
    };
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
#[ignore = "ADV-04: an edit that changes nothing still records an undo step"]
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
    let _ = host.perform(Command::SetNoteVelocity {
        track,
        clip: placement.id,
        note,
        velocity: 100,
    });
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
