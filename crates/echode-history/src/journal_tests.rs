//! Journal-tail regressions exercise the product append and repair paths.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_actions::{Command, JournalLine, JournalOp, Session, encode_line};
use echode_model::Project;

use super::{append_journal, recover, repair_journal, write_package};

thread_local! {
    // Each test owns its package and only observes reads on its own thread.
    static READ_BYTES: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn record_read(count: usize) {
    READ_BYTES.set(READ_BYTES.get() + count);
}

struct Package(PathBuf);

impl Package {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "echode-journal-tail-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        write_package(&directory, &Project::new()).unwrap();
        Self(directory)
    }

    fn journal(&self) -> PathBuf {
        self.0.join("journal.jsonl")
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn line(session: &mut Session, seq: u64, name: String) -> JournalLine {
    session.perform(Command::AddTrack { name }).unwrap();
    let entry = session.journal().last().unwrap();
    JournalLine {
        seq,
        op: JournalOp::Edit,
        actions: entry.actions.clone(),
        changes: entry.changes.clone().unwrap(),
    }
}

fn encoded(line: &JournalLine) -> Vec<u8> {
    format!("{}\n", encode_line(line)).into_bytes()
}

#[test]
fn audit_fca14_complete_journal_append_reads_only_the_last_byte() {
    // These are fixture sizes, not limits on record length or track names.
    for name_len in [1, 4096, 65_536] {
        let package = Package::new();
        let mut session = Session::open(Project::new());
        let first = line(&mut session, 1, "x".repeat(name_len));
        let next = line(&mut session, 2, "追加".into());
        let mut expected = encoded(&first);
        std::fs::write(package.journal(), &expected).unwrap();
        READ_BYTES.set(0);
        append_journal(&package.0, &next).unwrap();
        assert_eq!(
            READ_BYTES.get(),
            1,
            "a complete journal must inspect only its final byte (name length {name_len})"
        );
        expected.extend(encoded(&next));
        assert_eq!(std::fs::read(package.journal()).unwrap(), expected);
        assert_eq!(recover(&package.0).unwrap(), *session.project());
    }
}

#[test]
fn absent_and_empty_journals_need_no_read_before_the_first_append() {
    for absent in [false, true] {
        let package = Package::new();
        if !absent {
            std::fs::write(package.journal(), []).unwrap();
        }
        let mut session = Session::open(Project::new());
        let first = line(&mut session, 1, "first".into());
        READ_BYTES.set(0);
        append_journal(&package.0, &first).unwrap();
        assert_eq!(READ_BYTES.get(), 0);
        assert_eq!(std::fs::read(package.journal()).unwrap(), encoded(&first));
        assert_eq!(recover(&package.0).unwrap(), *session.project());
    }
}

#[test]
fn torn_suffixes_across_scan_boundaries_preserve_the_committed_prefix() {
    let package = Package::new();
    let mut session = Session::open(Project::new());
    // The intact prefix is deliberately much larger than the scan scratch.
    let first = line(&mut session, 1, "p".repeat(131_072));
    let next = line(&mut session, 2, "after repair".into());
    let prefix = encoded(&first);
    // Byte lengths cover both sides of multiple 4 KiB scan boundaries.
    for tail_len in [1, 2, 4095, 4096, 4097, 8192, 8193, 16_384] {
        let mut torn = prefix.clone();
        torn.extend(std::iter::repeat_n(b'x', tail_len - 1));
        torn.push(0xe3); // Also exercise an incomplete UTF-8 code point.
        std::fs::write(package.journal(), torn).unwrap();
        READ_BYTES.set(0);
        append_journal(&package.0, &next).unwrap();
        assert!(
            READ_BYTES.get() <= tail_len + 4096,
            "repair reread the committed prefix: {} bytes for a {tail_len}-byte tail",
            READ_BYTES.get()
        );
        let mut expected = prefix.clone();
        expected.extend(encoded(&next));
        assert_eq!(std::fs::read(package.journal()).unwrap(), expected);
        assert_eq!(recover(&package.0).unwrap(), *session.project());
    }
}

#[test]
fn wholly_uncommitted_journals_are_removed_without_a_length_limit() {
    let package = Package::new();
    let mut session = Session::open(Project::new());
    let first = line(&mut session, 1, "after empty repair".into());
    // Fixture lengths span several scratch chunks, not a product record limit.
    for len in [1, 4096, 4097, 16_385] {
        std::fs::write(package.journal(), vec![0xff; len]).unwrap();
        READ_BYTES.set(0);
        repair_journal(&package.0).unwrap();
        assert_eq!(READ_BYTES.get(), len);
        assert!(std::fs::read(package.journal()).unwrap().is_empty());
        append_journal(&package.0, &first).unwrap();
        assert_eq!(std::fs::read(package.journal()).unwrap(), encoded(&first));
        assert_eq!(recover(&package.0).unwrap(), *session.project());
    }
}

#[test]
fn sparse_journal_repair_keeps_offsets_beyond_u32() {
    use std::io::{Seek, SeekFrom, Write};

    let package = Package::new();
    // A sparse fixture exercises 64-bit offsets without allocating a huge log.
    let end = u64::from(u32::MAX) + 123;
    let mut file = std::fs::File::create(package.journal()).unwrap();
    file.set_len(end).unwrap();
    file.seek(SeekFrom::Start(end - 1)).unwrap();
    file.write_all(b"\n").unwrap();
    READ_BYTES.set(0);
    repair_journal(&package.0).unwrap();
    assert_eq!(READ_BYTES.get(), 1);
    assert_eq!(file.metadata().unwrap().len(), end);
    // A torn suffix must truncate to the same 64-bit offset, not its low bits.
    file.write_all(&[0xe3; 5000]).unwrap();
    READ_BYTES.set(0);
    repair_journal(&package.0).unwrap();
    assert!(READ_BYTES.get() <= 5000 + 4096);
    assert_eq!(file.metadata().unwrap().len(), end);
}

#[test]
fn a_complete_invalid_record_is_not_silently_repaired() {
    let package = Package::new();
    let original = b"not json\n";
    std::fs::write(package.journal(), original).unwrap();
    repair_journal(&package.0).unwrap();
    assert_eq!(std::fs::read(package.journal()).unwrap(), original);
    assert!(recover(&package.0).is_err());
}
