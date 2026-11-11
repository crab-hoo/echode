//! The NDJSON files and the thread that appends to the log file.
//!
//! [`append_lines`] is the one rotation rule for every diagnostic NDJSON file:
//! this log, the panic log (ADR 0238) and the app's audition log.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use crate::{FILE_LIMIT, LOGGER, QUEUE};

/// How long [`flush`] waits for the writer. A flush at quit must not hang it.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

pub(crate) enum Message {
    Line(Vec<u8>),
    /// Answered once every line queued before it is written.
    Flush(SyncSender<()>),
}

/// `<name>.1.<extension>`: where [`append_lines`] moves a full file.
pub fn previous(path: &Path) -> PathBuf {
    let extension = path.extension().and_then(|value| value.to_str());
    path.with_extension(extension.map_or_else(|| "1".to_owned(), |value| format!("1.{value}")))
}

/// Appends whole lines to `path`, creating its directory. When the lines would
/// take a non-empty file past `limit` bytes, the file first replaces
/// [`previous`], so each file stays within the limit unless one write exceeds it.
pub fn append_lines(path: &Path, lines: &[u8], limit: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if size > 0 && size.saturating_add(lines.len() as u64) > limit {
        std::fs::rename(path, previous(path))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(lines)
}

/// Starts the writer for `path`. `None` if the thread cannot start.
pub(crate) fn start(path: PathBuf) -> Option<SyncSender<Message>> {
    let (queue, incoming) = sync_channel(QUEUE);
    std::thread::Builder::new()
        .name("echode-log".into())
        .spawn(move || write(&path, &incoming, &LOGGER.dropped))
        .ok()?;
    Some(queue)
}

pub(crate) fn flush(queue: &SyncSender<Message>) {
    let (done, finished) = sync_channel(1);
    if queue.send(Message::Flush(done)).is_ok() {
        let _ = finished.recv_timeout(FLUSH_WAIT);
    }
}

/// Appends each batch of queued lines in one write. `dropped` counts records
/// the full queue refused. A failed write adds its lines to that count, and the
/// next batch that succeeds reports both, since nothing else can tell.
pub(crate) fn write(path: &Path, incoming: &Receiver<Message>, dropped: &AtomicU64) {
    let mut failed = 0u64;
    let mut batch = Vec::new();
    let mut flushes = Vec::new();
    while let Ok(first) = incoming.recv() {
        let mut next = Some(first);
        while let Some(message) = next {
            match message {
                Message::Line(line) => batch.extend_from_slice(&line),
                Message::Flush(done) => flushes.push(done),
            }
            // Empty ends the batch; a closed queue ends it too, then the loop.
            next = incoming.try_recv().ok();
        }
        // JSON escapes newlines inside a value, so each one ends a line.
        let lines = batch.iter().filter(|byte| **byte == b'\n').count() as u64;
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost != 0 || failed != 0 {
            batch.extend_from_slice(&loss(lost, failed));
        }
        match append_lines(path, &batch, FILE_LIMIT) {
            Ok(()) => failed = 0,
            Err(_) => {
                failed = failed.saturating_add(1);
                dropped.fetch_add(lost.saturating_add(lines), Ordering::Relaxed);
            }
        }
        batch.clear();
        for done in flushes.drain(..) {
            let _ = done.send(());
        }
    }
}

/// The line that says what the file is missing. Built here rather than
/// logged, so it cannot itself be dropped or loop through the queue.
fn loss(dropped: u64, failed: u64) -> Vec<u8> {
    crate::line::Line::new(
        "warn",
        "echode_log".to_owned(),
        "log records were lost".to_owned(),
        [
            ("dropped".to_owned(), dropped.into()),
            ("failed_writes".to_owned(), failed.into()),
        ]
        .into_iter()
        .collect(),
    )
    .json()
}
