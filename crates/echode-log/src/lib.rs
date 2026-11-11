//! The product log (ADR 0322).
//!
//! Product code on non-audio threads logs through the `log` facade, with
//! structured fields (`log::warn!(device:% = id; "...")`). This crate is the one
//! logger behind it, installed once by the app's C ABI or the CLI. A record
//! becomes one JSON line. The caller's thread mirrors it to the unified log
//! (`os_log`, subsystem [`SUBSYSTEM`]) and, for the CLI, to stderr. A writer
//! thread appends the lines to the NDJSON file that `make debug-report` copies.
//!
//! Never log on an audio thread: a record allocates, and `os_log` and the queue
//! may wait. The engine's audio thread hands its events to its control thread
//! through a ring, and the control thread logs them (`echode-engine`).
//!
//! Logging never waits for the file. When the writer falls behind, the bounded
//! queue drops records and the next line the writer appends counts them.

mod file;
mod line;
mod os;
pub mod testing;

pub use file::{append_lines, previous};

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

use log::LevelFilter;

/// The unified log subsystem. Console filters records with `subsystem:io.echode`.
pub const SUBSYSTEM: &str = "io.echode";

/// Past this many bytes a file moves to `<name>.1.<extension>` and a new one
/// starts. The limit bounds these diagnostic records only.
pub const FILE_LIMIT: u64 = 1 << 20;

/// Lines waiting for the writer thread. Bounds this diagnostic queue only;
/// further records are dropped and counted, never waited for.
const QUEUE: usize = 1024;

/// Where records go. Every field off writes nothing, except to a test capture.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The NDJSON file the writer thread appends to.
    pub file: Option<PathBuf>,
    /// Mirror each record to the unified log, on the caller's thread.
    pub os_log: bool,
    /// Print each record to stderr, on the caller's thread.
    pub stderr: bool,
}

struct Sinks {
    queue: Option<SyncSender<file::Message>>,
    os_log: bool,
    stderr: bool,
}

struct Logger {
    sinks: OnceLock<Sinks>,
    /// Records the full queue refused since the writer last reported them.
    dropped: AtomicU64,
}

static LOGGER: Logger = Logger {
    sinks: OnceLock::new(),
    dropped: AtomicU64::new(0),
};

/// Installs the process's sinks. Only the first call counts; it returns true.
///
/// The level is `info` unless `ECHODE_LOG` names another (`debug`, `warn`...).
pub fn install(options: Options) -> bool {
    let mut first = false;
    let has_file = options.file.is_some();
    let sinks = LOGGER.sinks.get_or_init(|| {
        first = true;
        Sinks {
            queue: options.file.and_then(file::start),
            os_log: options.os_log,
            stderr: options.stderr,
        }
    });
    if !first {
        return false;
    }
    let level = std::env::var("ECHODE_LOG")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(LevelFilter::Info);
    attach(level);
    if has_file && sinks.queue.is_none() {
        log::error!(target: "echode_log", "the log writer thread did not start; the log file stays empty");
    }
    log::info!(
        target: "echode_log",
        pid = std::process::id(),
        file = has_file,
        os_log = sinks.os_log,
        level:% = level;
        "log started"
    );
    true
}

/// Waits until the writer has appended every line logged before this call,
/// or a short bound passes. Does nothing without a file.
pub fn flush() {
    if let Some(queue) = LOGGER.sinks.get().and_then(|sinks| sinks.queue.as_ref()) {
        file::flush(queue);
    }
}

/// Points the facade at this logger and raises its level to at least `level`.
fn attach(level: LevelFilter) {
    // Another call already set this same logger; nothing else sets one.
    let _ = log::set_logger(&LOGGER);
    if level > log::max_level() {
        log::set_max_level(level);
    }
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        // The facade's maximum level already filtered the record.
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = line::Line::of(record);
        testing::keep(&line);
        let Some(sinks) = self.sinks.get() else {
            return;
        };
        if sinks.os_log {
            os::write(&line);
        }
        if sinks.stderr {
            // Nothing can report a failed write to stderr.
            let _ = writeln!(std::io::stderr().lock(), "echode: {}", line.text());
        }
        if let Some(queue) = &sinks.queue
            && queue.try_send(file::Message::Line(line.json())).is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {
        flush();
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
