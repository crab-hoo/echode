//! Reproductions from the 2026-11-02 audits (XCA-02, XCA-03, HIS-A1).
//!
//! Each test fails until its item is fixed; then remove its `#[ignore]`.
//! See docs/reviews/2026-11-02-fix-instructions.md for the fix conditions.

use std::path::{Path, PathBuf};

use echode_actions::{Command, JournalLine, JournalOp, Session, encode_line};
use echode_history::{DocumentHost, read_package, write_object, write_package};
use echode_model::{Id, Project, Tick};

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir()
        .join(format!(
            "echode-audit-durability-{}-{name}",
            std::process::id()
        ))
        .join("song.echode");
    let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
    directory
}

/// A short 16-bit mono WAV. Its length is this fixture's only constraint.
fn wav(path: &Path) {
    let samples: Vec<i16> = (0..480).map(|index| ((index % 48) * 600) as i16).collect();
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn tip(directory: &Path) -> gix::ObjectId {
    gix::open(directory)
        .unwrap()
        .find_reference("refs/echode/autosave/main")
        .unwrap()
        .id()
        .detach()
}

fn tip_seq(directory: &Path) -> u64 {
    let repo = gix::open(directory).unwrap();
    let commit = repo.find_commit(tip(directory)).unwrap();
    let message = String::from_utf8_lossy(commit.message_raw().unwrap()).into_owned();
    message
        .lines()
        .find_map(|line| line.strip_prefix("Echode-Journal:"))
        .and_then(|seq| seq.trim().parse().ok())
        .unwrap()
}

/// The journal line a session writes for `command` on `project`.
fn line(project: &Project, seq: u64, command: Command) -> String {
    let mut session = Session::open(project.clone());
    session.perform(command).unwrap();
    let entry = session.journal().last().unwrap().clone();
    let line = JournalLine {
        seq,
        op: JournalOp::Edit,
        actions: entry.actions,
        changes: entry.changes.unwrap(),
    };
    format!("{}\n", encode_line(&line))
}

fn add_track(host: &DocumentHost, name: &str) -> Id {
    host.perform(Command::AddTrack { name: name.into() })
        .unwrap();
    host.snapshot()
        .tracks()
        .iter()
        .find(|track| track.name == name)
        .unwrap()
        .id
}

fn names(project: &Project) -> Vec<String> {
    project
        .tracks()
        .iter()
        .map(|track| track.name.clone())
        .collect()
}

/// A tip whose ref update was lost while the journal had already dropped
/// the next line leaves a gap. Opening must not hide the later line.
#[test]
#[ignore = "audit XCA-02: a journal gap is replayed as if nothing were missing"]
fn audit_xca02_a_journal_gap_is_not_silently_dropped() {
    let directory = scratch("gap");
    write_package(&directory, &Project::new()).unwrap();
    {
        let host = DocumentHost::open(&directory).unwrap();
        add_track(&host, "first");
        add_track(&host, "second");
    }
    let seq = tip_seq(&directory);
    let repo = gix::open(&directory).unwrap();
    let parent = repo
        .find_commit(tip(&directory))
        .unwrap()
        .parent_ids()
        .next()
        .unwrap()
        .detach();
    let project = read_package(&directory).unwrap();
    let after = line(
        &project,
        seq + 2,
        Command::AddTrack {
            name: "synced".into(),
        },
    );
    std::fs::write(directory.join("journal.jsonl"), after).unwrap();
    std::fs::write(
        directory.join(".git/refs/echode/autosave/main"),
        format!("{parent}\n"),
    )
    .unwrap();
    match DocumentHost::open(&directory) {
        Err(_) => {}
        Ok(host) => assert!(
            host.recovery_notice().is_some(),
            "a synced journal line after a gap was dropped without notice: {:?}",
            names(&host.snapshot())
        ),
    }
}

/// An object the rename made visible before its bytes reached the disk is
/// empty. Importing the same audio again must repair it.
#[test]
#[ignore = "audit XCA-03: a damaged media object refuses the same audio again"]
fn audit_xca03_reimport_repairs_a_damaged_object() {
    let directory = scratch("object");
    let source = directory.parent().unwrap().join("tone.wav");
    wav(&source);
    write_package(&directory, &Project::new()).unwrap();
    let import = |host: &DocumentHost, name: &str| {
        let track = add_track(host, name);
        host.perform(Command::ImportAudio {
            track,
            start: Tick::new(0),
            path: source.display().to_string(),
        })
    };
    {
        let host = DocumentHost::open(&directory).unwrap();
        import(&host, "first").unwrap();
    }
    let objects = directory.join("media/objects");
    let object = std::fs::read_dir(&objects)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&object, b"").unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    import(&host, "again").expect("the same audio must replace a damaged object");
    let hash = object.file_name().unwrap().to_str().unwrap().to_string();
    assert!(echode_history::read_object(&directory, &hash).is_ok());
}

/// A journal line may name plugin state whose object did not survive. Open
/// replays it without reading the object, then the next autosave commits it.
#[test]
#[ignore = "audit HIS-A1: a missing state object becomes an unreadable tip after the next save"]
fn audit_his_a1_a_missing_state_object_does_not_lose_later_edits() {
    let directory = scratch("state");
    write_package(&directory, &Project::new()).unwrap();
    let device = {
        let host = DocumentHost::open(&directory).unwrap();
        let track = add_track(&host, "synth");
        host.perform(Command::AddDevice {
            track,
            plugin: "dev.echode.sine-pad".into(),
        })
        .unwrap();
        host.snapshot().tracks()[0]
            .devices
            .iter()
            .find(|device| device.plugin.is_some())
            .unwrap()
            .id
    };
    let hash = write_object(&directory, b"plugin bytes").unwrap();
    let project = read_package(&directory).unwrap();
    let state = line(
        &project,
        tip_seq(&directory) + 1,
        Command::SetPluginState {
            device,
            state: Some(hash.clone()),
        },
    );
    std::fs::write(directory.join("journal.jsonl"), state).unwrap();
    std::fs::remove_file(directory.join("media/objects").join(&hash)).unwrap();
    let opened = DocumentHost::open(&directory);
    if let Ok(host) = &opened
        && host.recovery_notice().is_none()
    {
        add_track(host, "after");
    } else {
        return;
    }
    drop(opened);
    let reopened = DocumentHost::open(&directory).unwrap();
    assert!(
        reopened.recovery_notice().is_none(),
        "an edit accepted after opening is behind an unreadable tip"
    );
    assert!(names(&reopened.snapshot()).contains(&"after".to_string()));
}
