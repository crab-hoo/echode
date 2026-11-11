//! M1: a refused package entry keeps its name, kind and link count as a typed
//! cause inside the `io::Error`, with the kind and sentence it had as text.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::{
    DocumentHost, EntryKind, PackageEntryError, append_journal, object_hash, read_object,
    read_package, write_object, write_package,
};
use echode_model::{NoteDraft, Project, TICKS_PER_QUARTER, Tick};

fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("echode-entries-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// The entry refusal anywhere in `error`'s chain, with its `io::Error` kind.
/// `io::Error::source` skips its own inner error, so each step also reads
/// the inner error of an `io::Error`.
fn entry(error: &(dyn std::error::Error + 'static)) -> Option<(ErrorKind, PackageEntryError)> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(refused) = io
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<PackageEntryError>())
        {
            return Some((io.kind(), refused.clone()));
        }
        current = error.source();
    }
    None
}

fn with_note() -> Project {
    let (project, track) = Project::new().add_track("piano");
    project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::new(TICKS_PER_QUARTER),
            },
        )
        .unwrap()
        .0
}

#[test]
fn a_missing_media_folder_is_a_typed_cause() {
    for folder in ["media", "objects"] {
        let directory = scratch();
        write_package(&directory, &Project::new()).unwrap();
        let hash = write_object(&directory, b"audio").unwrap();
        assert_eq!(hash, object_hash(b"audio"));
        let removed = if folder == "media" {
            directory.join("media")
        } else {
            directory.join("media/objects")
        };
        std::fs::remove_dir_all(removed).unwrap();
        let error = read_object(&directory, &hash).unwrap_err();
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            error
                .to_string()
                .ends_with(&format!(": {folder} is missing")),
            "{error}"
        );
        assert_eq!(
            entry(&error),
            Some((
                ErrorKind::NotFound,
                PackageEntryError::MissingFolder {
                    name: folder.into()
                }
            )),
            "a missing {folder} folder must retain its typed cause"
        );
    }
}

#[test]
fn a_hard_linked_journal_reports_its_link_count() {
    let root = scratch();
    let directory = root.join("project.echode");
    write_package(&directory, &Project::new()).unwrap();
    let outside = root.join("outside.jsonl");
    std::fs::write(&outside, b"").unwrap();
    std::fs::hard_link(&outside, directory.join("journal.jsonl")).unwrap();
    let error = append_journal(
        &directory,
        &echode_actions::JournalLine {
            seq: 1,
            op: echode_actions::JournalOp::Undo,
            actions: Vec::new(),
            changes: Default::default(),
        },
    )
    .unwrap_err();
    let preserved = std::fs::read(&outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(preserved.is_empty());
    assert!(
        error
            .to_string()
            .ends_with(": package file is not a regular unshared file"),
        "{error}"
    );
    assert_eq!(
        entry(&error),
        Some((
            ErrorKind::InvalidInput,
            PackageEntryError::NotRegularUnsharedFile {
                kind: EntryKind::File,
                links: 2,
            }
        )),
        "a shared journal must retain its typed link count"
    );
}

#[test]
fn an_entry_of_another_type_names_both_kinds() {
    let project = with_note();
    let track = format!("{}.json", project.tracks()[0].id);
    let cases: [(&str, OsString, EntryKind, EntryKind, &str); 3] = [
        (
            "symbolic link",
            track.clone().into(),
            EntryKind::SymbolicLink,
            EntryKind::File,
            "is a symbolic link",
        ),
        (
            "directory for a file",
            "project.json".into(),
            EntryKind::Directory,
            EntryKind::File,
            "has the wrong file type",
        ),
        (
            "file for a directory",
            "tracks".into(),
            EntryKind::File,
            EntryKind::Directory,
            "has the wrong file type",
        ),
    ];
    for (case, name, actual, expected, reason) in cases {
        let directory = scratch();
        write_package(&directory, &project).unwrap();
        match case {
            "symbolic link" => {
                let path = directory.join("tracks").join(&track);
                let outside = directory.join("outside.json");
                std::fs::rename(&path, &outside).unwrap();
                std::os::unix::fs::symlink(&outside, &path).unwrap();
            }
            "directory for a file" => {
                std::fs::remove_file(directory.join("project.json")).unwrap();
                std::fs::create_dir(directory.join("project.json")).unwrap();
            }
            _ => {
                std::fs::remove_dir_all(directory.join("tracks")).unwrap();
                std::fs::write(directory.join("tracks"), b"").unwrap();
            }
        }
        let read = read_package(&directory).unwrap_err();
        let opened = DocumentHost::open(&directory).err();
        let _ = std::fs::remove_dir_all(&directory);
        let shown = format!("{} {reason}", name.to_string_lossy());
        assert!(read.to_string().ends_with(&shown), "{case}: {read}");
        let typed = Some((
            ErrorKind::InvalidInput,
            PackageEntryError::WrongType {
                name,
                actual,
                expected,
            },
        ));
        assert_eq!(
            entry(&read),
            typed,
            "{case}: a wrong entry type must retain its typed cause"
        );
        let opened = opened.expect("a refused entry must not open");
        assert_eq!(entry(&opened), typed, "{case}: through DocumentHost::open");
    }
}
