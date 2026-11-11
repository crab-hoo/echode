//! The unified log. Each record goes out on the caller's thread, so Console
//! shows that thread and time. The category is the crate without `echode_`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use oslog::{Level, OsLog};

use crate::SUBSYSTEM;
use crate::line::Line;

/// One handle per category. Categories are crate names, a short fixed set.
static LOGS: OnceLock<Mutex<BTreeMap<String, OsLog>>> = OnceLock::new();

pub(crate) fn write(line: &Line) {
    let category = category(&line.target);
    let level = match line.level {
        "error" => Level::Error,
        "warn" => Level::Default,
        "info" => Level::Info,
        _ => Level::Debug,
    };
    let text = line.message_and_fields();
    let mut logs = LOGS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    logs.entry(category.to_owned())
        .or_insert_with(|| OsLog::new(SUBSYSTEM, category))
        .with_level(level, &text);
}

/// `engine` for `echode_engine::audio_log`; any other crate keeps its name.
pub(crate) fn category(target: &str) -> &str {
    let krate = target.split("::").next().unwrap_or(target);
    krate.strip_prefix("echode_").unwrap_or(krate)
}
