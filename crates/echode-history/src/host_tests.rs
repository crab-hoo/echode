use super::*;
use crate::document::JournalFault;
use crate::document::write_package;
use crate::recover;
use std::sync::atomic::{AtomicU64, Ordering};

fn scratch() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-host-{}-{n}", std::process::id()));
    // A failed earlier run can leave a package under a reused pid. Its
    // journal would replay into this test.
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn contract_failed_initial_autosave_returns_and_releases_the_package() {
    const CHILD: &str = "ECHODE_FAILED_INITIAL_AUTOSAVE";
    if let Some(directory) = std::env::var_os(CHILD) {
        let directory = std::path::PathBuf::from(directory);
        let error = DocumentHost::open(&directory)
            .err()
            .expect("invalid Git metadata must fail");
        assert!(!error.to_string().is_empty());
        assert_eq!(crate::read_package(&directory).unwrap(), Project::new());
        // Only this test's deliberately invalid metadata is removed.
        std::fs::remove_dir_all(directory.join(".git")).unwrap();
        let host = DocumentHost::open(&directory).expect("the failed open must release its lock");
        host.perform(Command::AddTrack {
            name: "recovered".into(),
        })
        .unwrap();
        assert_eq!(host.snapshot().tracks()[0].name, "recovered");
        drop(host);
        return;
    }
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    std::fs::create_dir(directory.join(".git")).unwrap();
    std::fs::create_dir(directory.join(".git/config")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD, &directory)
        .args([
            "--exact",
            "host::tests::contract_failed_initial_autosave_returns_and_releases_the_package",
            "--nocapture",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Bound a deadlock, not a performance measurement. The child owns no UI or hardware.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let completed = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        completed,
        "failed initial autosave deadlocked during shutdown"
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_edit_is_still_there_after_the_host_is_dropped() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "piano".into(),
    })
    .unwrap();
    assert_eq!(host.snapshot().tracks()[0].name, "piano");
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let project = again.snapshot();
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(project.tracks().len(), 1);
    assert_eq!(project.tracks()[0].name, "piano");
}

#[test]
fn restoring_an_older_autosave_records_a_new_version_and_undoes() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "one".into() })
        .unwrap();
    assert_eq!(host.revisions().unwrap().len(), 2);
    host.perform(Command::AddTrack { name: "two".into() })
        .unwrap();
    let revisions = host.revisions().unwrap();
    assert_eq!(revisions.len(), 3);
    // Newest first. The last commit is the empty base; the one before it
    // is the project after the first track.
    let after_first = &revisions[revisions.len() - 2];
    host.perform(Command::Restore {
        revision: after_first.id.clone(),
    })
    .unwrap();
    assert_eq!(host.snapshot().tracks().len(), 1);
    assert_eq!(host.snapshot().tracks()[0].name, "one");
    let kept = after_first.id.clone();
    host.undo().unwrap();
    assert_eq!(host.snapshot().tracks().len(), 2);
    host.redo().unwrap();
    assert_eq!(host.snapshot().tracks()[0].name, "one");
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let names: Vec<_> = again
        .snapshot()
        .tracks()
        .iter()
        .map(|track| track.name.clone())
        .collect();
    let still = again.revisions().unwrap();
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(names, ["one"]);
    assert!(still.iter().any(|revision| revision.id == kept));
}

fn names(host: &DocumentHost) -> Vec<String> {
    host.snapshot()
        .tracks()
        .iter()
        .map(|track| track.name.clone())
        .collect()
}

#[test]
fn audit_aud01_a_second_open_in_this_process_is_rejected() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "A".into() })
        .unwrap();
    let second = DocumentHost::open(&directory);
    assert!(second.is_err(), "a second writer must be rejected");
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(recovered, ["A"]);
}

#[test]
fn audit_aud01_a_symlink_is_the_same_package() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let link = scratch();
    std::os::unix::fs::symlink(&directory, &link).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    let through_link = DocumentHost::open(&link);
    drop(host);
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(directory);
    assert!(
        through_link.is_err(),
        "a symlink must not open a second writer"
    );
}

#[test]
fn audit_aud01_another_process_cannot_open_and_a_forced_exit_keeps_the_edit() {
    if let Ok(directory) = std::env::var("ECHODE_AUDIT_HOLD") {
        let host = DocumentHost::open(std::path::Path::new(&directory)).unwrap();
        println!("holding");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut byte = [0u8; 1];
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut byte);
        host.perform(Command::AddTrack {
            name: "kept".into(),
        })
        .unwrap();
        // Safety: the journal line was synced before this exit. Drop is not required.
        unsafe { libc::_exit(0) };
    }
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("ECHODE_AUDIT_HOLD", &directory)
        .arg("--exact")
        .arg(
            "host::tests::audit_aud01_another_process_cannot_open_and_a_forced_exit_keeps_the_edit",
        )
        .arg("--nocapture")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut seen = String::new();
    let mut buffer = [0u8; 64];
    while !seen.contains("holding") {
        let read = std::io::Read::read(&mut stdout, &mut buffer).unwrap();
        assert!(read > 0, "the holder exited before it owned the package");
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }
    let rejected = DocumentHost::open(&directory);
    let link = scratch();
    std::os::unix::fs::symlink(&directory, &link).unwrap();
    let rejected_link = DocumentHost::open(&link);
    assert!(
        rejected.is_err(),
        "another process must not become a writer"
    );
    assert!(rejected_link.is_err(), "a symlink must not become a writer");
    let _ = std::fs::remove_file(&link);
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &[1]).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(recovered, ["kept"]);
}

#[test]
fn audit_aud02_every_torn_offset_can_be_edited_and_reopened() {
    let source = scratch();
    write_package(&source, &Project::new()).unwrap();
    let full = {
        let host = DocumentHost::open(&source).unwrap();
        host.perform(Command::AddTrack {
            name: "ピアノ".into(),
        })
        .unwrap();
        let bytes = std::fs::read(source.join("journal.jsonl")).unwrap();
        drop(host);
        bytes
    };
    let _ = std::fs::remove_dir_all(source);
    assert!(full.ends_with(b"\n"));
    let text = std::str::from_utf8(&full).unwrap();
    assert!(text.contains("ピアノ"));
    let line = echode_actions::decode_line(text.lines().next().unwrap()).unwrap();
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    {
        let host = DocumentHost::open(&directory).unwrap();
        drop(host);
    }
    let log = directory.join("journal.jsonl");
    // Each offset is repaired, reopened, and given one more synced record.
    // DocumentHost::open and perform use these same steps.
    for cut in 0..=full.len() {
        std::fs::write(&log, &full[..cut]).unwrap();
        crate::document::repair_journal(&directory).unwrap();
        let opened = recover(&directory).unwrap();
        if !opened.tracks().iter().any(|track| track.name == "ピアノ") {
            crate::document::append_journal(&directory, &line).unwrap();
        }
        let again = recover(&directory).unwrap();
        assert!(
            again.tracks().iter().any(|track| track.name == "ピアノ"),
            "cut {cut} of {} dropped the committed edit",
            full.len()
        );
    }
    let mut torn = full.clone();
    torn.push(0xE3);
    std::fs::write(&log, &torn).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "確定".into(),
    })
    .unwrap();
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert!(recovered.iter().any(|name| name == "ピアノ"));
    assert!(recovered.iter().any(|name| name == "確定"));
}

#[test]
fn audit_aud03_a_failed_journal_write_is_not_part_of_the_next_edit() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    let log = directory.join("journal.jsonl");
    if log.exists() {
        std::fs::remove_file(&log).unwrap();
    }
    std::fs::create_dir(&log).unwrap();
    let failed = host.perform(Command::AddTrack {
        name: "rejected".into(),
    });
    assert!(failed.is_err());
    assert!(names(&host).is_empty());
    std::fs::remove_dir(&log).unwrap();
    host.perform(Command::AddTrack {
        name: "accepted".into(),
    })
    .unwrap();
    assert_eq!(names(&host), ["accepted"]);
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(recovered, ["accepted"]);
}

#[test]
fn audit_aud03_a_partial_write_and_a_sync_failure_roll_back() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "kept".into(),
    })
    .unwrap();
    for fault in [
        JournalFault::Partial,
        JournalFault::Sync,
        JournalFault::BeforeWrite,
    ] {
        crate::document::fail_next_journal(host.directory(), fault);
        let failed = host.perform(Command::AddTrack {
            name: "rejected".into(),
        });
        assert!(failed.is_err(), "{fault:?}");
        assert_eq!(names(&host), ["kept"]);
        host.undo().unwrap();
        assert!(names(&host).is_empty());
        host.redo().unwrap();
        assert_eq!(names(&host), ["kept"]);
    }
    host.perform(Command::AddTrack {
        name: "accepted".into(),
    })
    .unwrap();
    assert_eq!(names(&host), ["kept", "accepted"]);
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(recovered, ["kept", "accepted"]);
}

#[test]
fn audit_aud03_unknown_durability_stops_later_edits_until_recovery() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    crate::document::fail_next_journal(host.directory(), JournalFault::Uncertain);
    let failed = host.perform(Command::AddTrack {
        name: "landed".into(),
    });
    assert!(
        failed
            .unwrap_err()
            .to_string()
            .contains("journal durability is unknown")
    );
    assert!(names(&host).is_empty());
    let next = host.perform(Command::AddTrack {
        name: "mixed".into(),
    });
    assert!(
        next.unwrap_err()
            .to_string()
            .contains("journal durability is unknown")
    );
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let recovered = names(&again);
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(recovered, ["landed"]);
}

#[test]
fn audit_rea09_export_label_uses_readable_parent() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack {
        name: "piano".into(),
    })
    .unwrap();
    let parent = host.revisions().unwrap().into_iter().next().unwrap().id;
    host.perform(Command::AddTrack {
        name: "drums".into(),
    })
    .unwrap();
    let tip = host.revisions().unwrap().into_iter().next().unwrap().id;
    assert_ne!(parent, tip);
    drop(host);
    let repo = gix::open(&directory).unwrap();
    let commit = repo
        .find_commit(gix::ObjectId::from_hex(tip.as_bytes()).unwrap())
        .unwrap();
    let blob = commit
        .tree()
        .unwrap()
        .lookup_entry_by_path("project.json")
        .unwrap()
        .unwrap()
        .object_id();
    let hex = blob.to_string();
    std::fs::remove_file(
        directory
            .join(".git/objects")
            .join(&hex[..2])
            .join(&hex[2..]),
    )
    .unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    let (id, _) = host.recovered_label().unwrap();
    assert_eq!(host.snapshot().tracks().len(), 1);
    assert_eq!(host.snapshot().tracks()[0].name, "piano");
    assert_eq!(id, parent);
    assert_ne!(id, tip);
    drop(host);
    let _ = std::fs::remove_dir_all(directory);
}

fn clap(project: &Project) -> &echode_model::Device {
    project.tracks()[0]
        .devices
        .iter()
        .find(|device| device.plugin.as_deref() == Some("dev.echode.capture"))
        .unwrap()
}

#[test]
fn a_captured_plugin_state_keeps_redo_and_does_not_add_an_undo_step() {
    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = host.snapshot().tracks()[0].id;
    host.perform(Command::AddDevice {
        track,
        plugin: "dev.echode.capture".into(),
    })
    .unwrap();
    host.perform(Command::AddTrack { name: "b".into() })
        .unwrap();
    host.undo().unwrap();
    assert_eq!(host.snapshot().tracks().len(), 1);
    let device = clap(&host.snapshot()).id;
    assert!(host.commit_plugin_state(device, b"captured-state").unwrap());
    host.redo().unwrap();
    assert_eq!(host.snapshot().tracks().len(), 2);
    assert_eq!(host.snapshot().tracks()[1].name, "b");
    drop(host);
    let _ = std::fs::remove_dir_all(directory);

    let directory = scratch();
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    host.perform(Command::AddTrack { name: "a".into() })
        .unwrap();
    let track = host.snapshot().tracks()[0].id;
    host.perform(Command::AddDevice {
        track,
        plugin: "dev.echode.capture".into(),
    })
    .unwrap();
    let device = clap(&host.snapshot()).id;
    assert!(host.commit_plugin_state(device, b"captured-state").unwrap());
    let saved = clap(&host.snapshot()).state.clone();
    host.undo().unwrap();
    assert!(
        host.snapshot().tracks()[0]
            .devices
            .iter()
            .all(|device| device.plugin.is_none())
    );
    host.undo().unwrap();
    assert!(host.snapshot().tracks().is_empty());
    assert!(host.undo().is_err());
    host.redo().unwrap();
    host.redo().unwrap();
    drop(host);
    let again = DocumentHost::open(&directory).unwrap();
    let project = again.snapshot();
    drop(again);
    let _ = std::fs::remove_dir_all(directory);
    assert_eq!(clap(&project).state, saved);
}
