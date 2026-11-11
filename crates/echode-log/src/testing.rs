//! Lets a test read the records its product path logged.
//!
//! [`capture`] points the facade at this crate's logger and keeps every later
//! record in memory, whatever other sinks are installed. Tests in one binary
//! share it, so each test picks its own records by a field it alone uses, such
//! as a fresh device id.

use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::line::Line;

static KEPT: OnceLock<Mutex<Vec<Value>>> = OnceLock::new();

/// Records logged in this process from the first `capture` on.
pub struct Capture(&'static Mutex<Vec<Value>>);

/// Starts keeping records at every level. Later calls return the same capture.
pub fn capture() -> Capture {
    let kept = KEPT.get_or_init(Mutex::default);
    crate::attach(log::LevelFilter::Trace);
    Capture(kept)
}

impl Capture {
    /// Every record kept so far, as the file's JSON objects.
    pub fn records(&self) -> Vec<Value> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The records logged on the calling thread. The test harness names each
    /// test's thread after the test, so this is that test's own records.
    pub fn on_this_thread(&self) -> Vec<Value> {
        let thread = std::thread::current();
        let Some(name) = thread.name() else {
            return Vec::new();
        };
        self.records()
            .into_iter()
            .filter(|record| record["thread"] == name)
            .collect()
    }

    /// The records whose field `key` equals `value`.
    pub fn with_field(&self, key: &str, value: &Value) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|record| record["fields"][key] == *value)
            .collect()
    }
}

pub(crate) fn keep(line: &Line) {
    if let Some(kept) = KEPT.get() {
        kept.lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(line.value());
    }
}
