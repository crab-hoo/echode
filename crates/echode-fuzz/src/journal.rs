//! `journal.jsonl` replayed over a saved package, the way `recover` opens a
//! document after a crash.
//!
//! Input: the bytes of `journal.jsonl`. The package under it is the sample
//! package, written once per thread, so each input runs only the replay.

use std::path::{Path, PathBuf};

use echode_history::recover;

use crate::package::assert_saves;
use crate::sample;
use crate::scratch;

struct Base {
    package: PathBuf,
    /// Holds a copy of the base store, so a replayed project's plugin states
    /// read back after saving it here.
    saved: PathBuf,
}

impl Base {
    fn new() -> Self {
        let package = scratch::kept("journal-base");
        crate::package::unpack(&package, &sample::sample().archive);
        let saved = scratch::kept("journal-saved");
        copy_store(&package, &saved);
        Self { package, saved }
    }
}

thread_local! {
    static BASE: Base = Base::new();
}

pub(crate) fn run(data: &[u8]) {
    BASE.with(|base| {
        std::fs::write(base.package.join("journal.jsonl"), data).expect("write the journal");
        if let Ok(project) = recover(&base.package) {
            assert_saves(&base.saved, &project);
        }
    });
}

fn copy_store(from: &Path, to: &Path) {
    let objects = Path::new("media").join("objects");
    std::fs::create_dir_all(to.join(&objects)).expect("create the store");
    for entry in std::fs::read_dir(from.join(&objects)).expect("list the store") {
        let entry = entry.expect("a store entry");
        std::fs::copy(entry.path(), to.join(&objects).join(entry.file_name()))
            .expect("copy a store object");
    }
}

/// Each prefix of the sample journal: the lines number from 1, so every
/// prefix replays in full.
pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let journal = &sample::sample().journal;
    let mut seeds = Vec::new();
    let mut prefix = String::new();
    for line in journal.lines() {
        prefix.push_str(line);
        prefix.push('\n');
        seeds.push(prefix.clone().into_bytes());
    }
    seeds
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_sample_journal_replays_over_the_base_package() {
        super::BASE.with(|base| {
            let journal = &super::sample::sample().journal;
            std::fs::write(base.package.join("journal.jsonl"), journal).unwrap();
            let recovered = echode_history::recover(&base.package).unwrap();
            let lines = journal.lines().count();
            assert!(lines >= 6, "{lines} lines");
            let base_project = echode_history::read_package(&base.package).unwrap();
            assert_ne!(recovered, base_project, "the journal changed nothing");
            super::assert_saves(&base.saved, &recovered);
        });
    }
}
