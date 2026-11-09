//! Package directories for one input at a time.
//!
//! Each thread has its own root, so the replay test's parallel threads never
//! share a package. `ECHODE_FUZZ_SCRATCH` moves the roots under one directory,
//! which `make fuzz` removes after the run.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static THREADS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static ROOT: PathBuf = {
        let base = std::env::var_os("ECHODE_FUZZ_SCRATCH")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join(format!(
            "echode-fuzz-{}-{}",
            std::process::id(),
            THREADS.fetch_add(1, Ordering::Relaxed)
        ))
    };
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// A directory that does not exist yet, removed again when dropped.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        let index = NEXT.with(|next| {
            let index = next.get();
            next.set(index + 1);
            index
        });
        let path = ROOT.with(|root| root.join(format!("{name}-{index}")));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A directory kept for the thread's lifetime, such as a base package.
pub(crate) fn kept(name: &str) -> PathBuf {
    let path = ROOT.with(|root| root.join(name));
    let _ = std::fs::remove_dir_all(&path);
    path
}

pub(crate) fn remove_thread_root() {
    ROOT.with(|root| {
        let _ = std::fs::remove_dir_all(root);
    });
}
