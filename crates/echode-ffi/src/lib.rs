//! C ABI for the app shell.
//!
//! A command is queued and the caller returns. The document thread applies it,
//! then the completion callback runs once, without a lock held. The spike in
//! `spikes/bridge` is not used here.

mod queue;

pub use queue::{Document, Snapshot};

use std::ffi::{CStr, c_char, c_void};
use std::os::raw::c_int;

use echode_history::write_package;
use echode_model::Project;

/// The command was applied. `snapshot` is owned by the caller.
pub const ECHODE_OK: c_int = 0;
/// The command was rejected, or the document thread has stopped.
pub const ECHODE_ERR: c_int = 1;

/// Called once for every accepted submission. `message` is valid only for this
/// call. On success, `snapshot` is owned by the caller.
pub type Complete = extern "C" fn(
    context: *mut c_void,
    status: c_int,
    message: *const c_char,
    snapshot: *mut Snapshot,
);

/// Creates an empty package at `path` and opens it. Null if that fails.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. The returned pointer is freed
/// with [`echode_document_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_create(path: *const c_char) -> *mut Document {
    let Some(path) = c_str(path) else {
        return std::ptr::null_mut();
    };
    let path = std::path::Path::new(&path);
    if write_package(path, &Project::new()).is_err() {
        return std::ptr::null_mut();
    }
    Document::open(path)
        .map(|document| Box::into_raw(Box::new(document)))
        .unwrap_or(std::ptr::null_mut())
}

/// Opens a package. Null if `path` is not a recoverable package.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. The returned pointer is freed
/// with [`echode_document_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_open(path: *const c_char) -> *mut Document {
    let Some(path) = c_str(path) else {
        return std::ptr::null_mut();
    };
    Document::open(std::path::Path::new(&path))
        .map(|document| Box::into_raw(Box::new(document)))
        .unwrap_or(std::ptr::null_mut())
}

/// Finishes queued commands, then rejects later submissions.
///
/// # Safety
/// `document` came from [`echode_document_open`] and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_shutdown(document: *mut Document) {
    if let Some(document) = document_ref(document) {
        document.shutdown();
    }
}

/// Drops the document. A snapshot taken earlier stays readable.
///
/// # Safety
/// `document` came from [`echode_document_open`] and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_close(document: *mut Document) {
    if !document.is_null() {
        drop(unsafe { Box::from_raw(document) });
    }
}

/// Queues `json` and returns. `complete` runs once with `context`.
///
/// `complete` is required and runs once, which hands `context` back.
///
/// # Safety
/// `document` is open. `json` is a nullable byte string of `json_len` bytes,
/// not necessarily NUL-terminated. `context` is not read by Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_submit(
    document: *mut Document,
    json: *const u8,
    json_len: usize,
    context: *mut c_void,
    complete: Complete,
) -> c_int {
    let Some(document) = document_ref(document) else {
        complete(
            context,
            ECHODE_ERR,
            c"document is null".as_ptr(),
            std::ptr::null_mut(),
        );
        return 0;
    };
    let bytes = if json.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_len) }
    };
    document.submit(bytes, context, complete);
    0
}

/// Another owned handle for the same project.
///
/// # Safety
/// `snapshot` came from a completion or from this function and has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_retain(snapshot: *mut Snapshot) -> *mut Snapshot {
    snapshot_ref(snapshot)
        .map(|snapshot| Box::into_raw(Box::new(snapshot.retain())))
        .unwrap_or(std::ptr::null_mut())
}

/// Drops one owned handle.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_free(snapshot: *mut Snapshot) {
    if !snapshot.is_null() {
        drop(unsafe { Box::from_raw(snapshot) });
    }
}

/// How many tracks the snapshot contains. Null is zero.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_count(snapshot: *const Snapshot) -> u32 {
    snapshot_ref(snapshot)
        .map(Snapshot::track_count)
        .unwrap_or(0)
}

fn c_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_string)
}

fn document_ref<'a>(document: *mut Document) -> Option<&'a Document> {
    if document.is_null() {
        None
    } else {
        Some(unsafe { &*document })
    }
}

fn snapshot_ref<'a>(snapshot: *const Snapshot) -> Option<&'a Snapshot> {
    if snapshot.is_null() {
        None
    } else {
        Some(unsafe { &*snapshot })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_history::write_package;
    use echode_model::Project;
    use std::ffi::CStr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;

    struct Reply {
        sender: mpsc::Sender<Outcome>,
    }

    struct Outcome {
        status: c_int,
        message: Option<String>,
        snapshot: *mut Snapshot,
    }

    extern "C" fn on_complete(
        context: *mut c_void,
        status: c_int,
        message: *const c_char,
        snapshot: *mut Snapshot,
    ) {
        let reply = unsafe { &*(context as *const Reply) };
        let message = if message.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(message) }
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        reply
            .sender
            .send(Outcome {
                status,
                message,
                snapshot,
            })
            .expect("test is waiting");
    }

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("echode-ffi-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        write_package(&path, &Project::new()).unwrap();
        path
    }

    fn open() -> *mut Document {
        let path = scratch();
        let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let document = unsafe { echode_document_open(c_path.as_ptr()) };
        assert!(!document.is_null());
        document
    }

    fn submit(
        document: *mut Document,
        json: &str,
        reply: &Reply,
        incoming: &mpsc::Receiver<Outcome>,
    ) -> Outcome {
        let code = unsafe {
            echode_document_submit(
                document,
                json.as_ptr(),
                json.len(),
                reply as *const Reply as *mut c_void,
                on_complete,
            )
        };
        assert_eq!(code, 0);
        incoming
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("completion")
    }

    #[test]
    fn a_command_returns_before_its_snapshot_is_freed_with_the_document() {
        let document = open();
        let (sender, incoming) = mpsc::channel();
        let reply = Reply { sender };
        let outcome = submit(
            document,
            r#"{"type":"track.add","name":"piano"}"#,
            &reply,
            &incoming,
        );
        assert_eq!(outcome.status, ECHODE_OK);
        assert!(outcome.message.is_none());
        assert_eq!(unsafe { echode_snapshot_track_count(outcome.snapshot) }, 1);
        assert!(incoming.try_recv().is_err());
        unsafe { echode_document_close(document) };
        assert_eq!(unsafe { echode_snapshot_track_count(outcome.snapshot) }, 1);
        let retained = unsafe { echode_snapshot_retain(outcome.snapshot) };
        unsafe { echode_snapshot_free(outcome.snapshot) };
        assert_eq!(unsafe { echode_snapshot_track_count(retained) }, 1);
        unsafe { echode_snapshot_free(retained) };
    }

    #[test]
    fn a_bad_command_and_a_stopped_document_complete_once() {
        let document = open();
        let (sender, incoming) = mpsc::channel();
        let reply = Reply { sender };
        let outcome = submit(document, "{", &reply, &incoming);
        assert_eq!(outcome.status, ECHODE_ERR);
        assert!(outcome.snapshot.is_null());
        assert!(outcome.message.is_some());
        assert!(incoming.try_recv().is_err());
        unsafe { echode_document_shutdown(document) };
        let outcome = submit(
            document,
            r#"{"type":"track.add","name":"late"}"#,
            &reply,
            &incoming,
        );
        assert_eq!(outcome.status, ECHODE_ERR);
        assert!(outcome.message.unwrap().contains("stopped"));
        unsafe { echode_document_close(document) };
    }

    #[test]
    fn the_generated_header_names_the_entry_points() {
        let header = include_str!(concat!(env!("OUT_DIR"), "/echode.h"));
        assert!(header.contains("echode_document_submit"));
        assert!(header.contains("echode_document_create"));
        assert!(header.contains("echode_snapshot_free"));
        assert!(header.contains("Generated by cbindgen"));
        assert!(header.contains("typedef void (*Complete)"));
    }
}
