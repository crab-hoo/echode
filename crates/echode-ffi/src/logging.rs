//! The product log through the C ABI (ADR 0322). The app installs it once,
//! writes its own records into it, and keeps its audition log under the same
//! rotation rule.

use std::ffi::c_char;
use std::path::PathBuf;

use crate::abi;

/// [`echode_log_write`] levels.
pub const ECHODE_LOG_ERROR: u32 = 1;
pub const ECHODE_LOG_WARN: u32 = 2;
pub const ECHODE_LOG_INFO: u32 = 3;
pub const ECHODE_LOG_DEBUG: u32 = 4;

/// The size past which every diagnostic NDJSON file moves aside. Written out
/// for the C header; the build fails if it parts from `echode_log::FILE_LIMIT`.
pub const ECHODE_LOG_FILE_LIMIT: u64 = 1_048_576;
const _: () = assert!(ECHODE_LOG_FILE_LIMIT == echode_log::FILE_LIMIT);

/// Installs the process's log. Records go to the unified log, and the lines to
/// the NDJSON file at `path` when it is not null. Only the first call counts.
///
/// # Safety
/// `path` is a nullable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_log_install(path: *const c_char) {
    // SAFETY: the caller passes null or a NUL-terminated string.
    let file = unsafe { abi::text(path) }.map(PathBuf::from);
    echode_log::install(echode_log::Options {
        file,
        os_log: true,
        stderr: false,
    });
}

/// Logs `message` from the app with the target `app::<category>`. False for
/// an unknown level or a missing or invalid string. Never call it on an audio
/// thread.
///
/// # Safety
/// `category` and `message` are nullable NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_log_write(
    level: u32,
    category: *const c_char,
    message: *const c_char,
) -> bool {
    let level = match level {
        ECHODE_LOG_ERROR => log::Level::Error,
        ECHODE_LOG_WARN => log::Level::Warn,
        ECHODE_LOG_INFO => log::Level::Info,
        ECHODE_LOG_DEBUG => log::Level::Debug,
        _ => return false,
    };
    // SAFETY: the caller passes null or NUL-terminated strings.
    let (Some(category), Some(message)) = (unsafe { abi::text(category) }, unsafe {
        abi::text(message)
    }) else {
        return false;
    };
    log::log!(target: &format!("app::{category}"), level, "{message}");
    true
}

/// Waits until the log file holds every record logged before this call, or
/// two seconds pass.
#[unsafe(no_mangle)]
pub extern "C" fn echode_log_flush() {
    echode_log::flush();
}

/// Appends `len` bytes of whole NDJSON lines to the file at `path`, under the
/// rotation rule every diagnostic log shares: past `limit` bytes the file moves
/// to `<name>.1.<extension>` first. The app passes [`ECHODE_LOG_FILE_LIMIT`];
/// a test may pass less. False if an argument is refused or the write fails.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. A non-null `bytes` addresses
/// `len` readable bytes that nothing writes during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_log_append_lines(
    path: *const c_char,
    bytes: *const u8,
    len: usize,
    limit: u64,
) -> bool {
    // SAFETY: the caller passes null or a NUL-terminated string.
    let Some(path) = (unsafe { abi::text(path) }) else {
        return false;
    };
    // SAFETY: the caller keeps `len` bytes readable and unwritten during the call.
    let Some(lines) = (unsafe { abi::input(bytes, len) }) else {
        return false;
    };
    echode_log::append_lines(std::path::Path::new(&path), lines, limit).is_ok()
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
