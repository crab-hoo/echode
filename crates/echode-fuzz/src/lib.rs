//! Fuzz targets for input that Echode reads from other processes and from disk
//! (XCA-17, ADR 0290).
//!
//! Each target hands bytes to the product entry point that reads that input:
//!
//! - [`Target::Control`]: the control socket a plugin helper writes to.
//! - [`Target::Region`]: the shared memory a plugin helper can overwrite.
//! - [`Target::Journal`]: `journal.jsonl` replayed over a saved package.
//! - [`Target::Package`]: a package's JSON files, store objects and journal.
//! - [`Target::Audio`]: stored PCM objects and audio files to import.
//!
//! A panic is a finding. Besides crashes, each target checks a property that
//! must hold for any input the product accepts: a decoded control message
//! encodes to the same message, a recovered project keeps its invariants and
//! saves and reads back unchanged, and decoded audio is finite and complete.
//!
//! The libFuzzer binaries in `fuzz_targets/` and the replay test in
//! `tests/replay.rs` call the same [`Target::run`], so every input a fuzzer
//! found and that was saved under `regressions/` stays a normal test.

mod audio;
mod bytes;
mod control;
mod journal;
mod package;
mod region;
mod sample;
mod scratch;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// One kind of external input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Control,
    Region,
    Journal,
    Package,
    Audio,
}

impl Target {
    pub const ALL: [Self; 5] = [
        Self::Control,
        Self::Region,
        Self::Journal,
        Self::Package,
        Self::Audio,
    ];

    /// The libFuzzer binary and the directory names under `regressions/`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Region => "region",
            Self::Journal => "journal",
            Self::Package => "package",
            Self::Audio => "audio",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|target| target.name() == name)
    }

    /// Feeds one input to the product. Panics when the product crashes or
    /// breaks the target's property.
    ///
    /// Runs one input at a time in the process: the control target counts the
    /// process's open descriptors, which another thread's work would change.
    pub fn run(self, data: &[u8]) {
        let _turn = one_at_a_time();
        match self {
            Self::Control => control::run(data),
            Self::Region => region::run(data),
            Self::Journal => journal::run(data),
            Self::Package => package::run(data),
            Self::Audio => audio::run(data),
        }
    }

    /// Valid inputs written by the product's own encoders, so a fuzzer starts
    /// from the formats as they are now rather than from stale files.
    pub fn seeds(self) -> Vec<Vec<u8>> {
        let _turn = one_at_a_time();
        match self {
            Self::Control => control::seeds(),
            Self::Region => region::seeds(),
            Self::Journal => journal::seeds(),
            Self::Package => package::seeds(),
            Self::Audio => audio::seeds(),
        }
    }
}

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    // A finding panics while holding the lock; the next input still runs.
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes this thread's scratch packages. The fuzzer binaries leave theirs to
/// `make fuzz`, which removes the whole scratch directory after the run.
pub fn remove_scratch() {
    scratch::remove_thread_root();
}
