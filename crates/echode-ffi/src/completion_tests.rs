//! Every accepted submission completes exactly once, whichever thread ends it.
//! A callback may shut down or close its document from any of those threads.

use super::*;
use echode_history::write_package;
use echode_model::{Id, Project};
use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

fn package() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("echode-ffi-completion-{}", Id::generate()));
    write_package(&path, &Project::new()).unwrap();
    path
}

struct Closing {
    document: *mut Document,
    close: bool,
    sender: mpsc::Sender<c_int>,
}

extern "C" fn close_from_completion(
    context: *mut c_void,
    status: c_int,
    _: *const c_char,
    snapshot: *mut Snapshot,
) {
    // The callback owns its context: the waiter may return while this runs.
    let reply = unsafe { Box::from_raw(context.cast::<Closing>()) };
    unsafe { echode_snapshot_free(snapshot) };
    unsafe {
        if reply.close {
            echode_document_close(reply.document);
        } else {
            echode_document_shutdown(reply.document);
        }
    }
    let _ = reply.sender.send(status);
}

/// Counts joins that macOS refused because two threads were joining each
/// other. Whichever side joins second panics: the worker while it tears down,
/// or the callback's C entry point, which aborts the process.
fn refused_joins() -> &'static AtomicUsize {
    static REFUSED: OnceLock<AtomicUsize> = OnceLock::new();
    REFUSED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.to_string().contains("failed to join thread") {
                refused_joins().fetch_add(1, Ordering::SeqCst);
            }
            previous(info);
        }));
        AtomicUsize::new(0)
    })
}

/// A window close with no helper completes from the device lane.
fn complete_a_device_request_by(close: bool) {
    let path = package();
    let document = Box::into_raw(Box::new(Document::open(&path).unwrap()));
    let (sender, incoming) = mpsc::channel::<c_int>();
    let reply = Box::into_raw(Box::new(Closing {
        document,
        close,
        sender,
    }));
    let json = format!(
        r#"{{"type":"device.gui.close","device":"{}"}}"#,
        Id::from_raw(0xF0)
    );
    let code = unsafe {
        echode_document_device(
            document,
            json.as_ptr(),
            json.len(),
            reply.cast(),
            close_from_completion,
        )
    };
    assert_eq!(code, ECHODE_OK);
    let status = incoming
        .recv_timeout(WAIT)
        .expect("the callback ended its document without waiting for its own thread");
    assert_eq!(status, ECHODE_OK);
    if !close {
        unsafe { echode_document_close(document) };
    }
    // The worker finishes after the callback returns. Reopening proves it let
    // the package go and that its teardown did not fail.
    let reopened = wait_for_reopen(&path);
    unsafe { echode_document_close(reopened) };
    let _ = std::fs::remove_dir_all(path);
}

fn wait_for_reopen(path: &std::path::Path) -> *mut Document {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(document) = Document::open(path) {
            return Box::into_raw(Box::new(document));
        }
        assert!(Instant::now() < deadline, "the package was never released");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn contract_ending_a_document_from_a_device_completion_never_joins_its_own_thread() {
    let refused = refused_joins();
    let before = refused.load(Ordering::SeqCst);
    for round in 0..4 {
        complete_a_device_request_by(round % 2 == 0);
    }
    assert_eq!(
        refused.load(Ordering::SeqCst),
        before,
        "the worker and the lane joined each other"
    );
}

extern "C" fn report_status(
    context: *mut c_void,
    status: c_int,
    message: *const c_char,
    snapshot: *mut Snapshot,
) {
    let sender = unsafe { Box::from_raw(context.cast::<mpsc::Sender<(c_int, Option<String>)>>()) };
    let message = (!message.is_null()).then(|| {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    });
    unsafe { echode_snapshot_free(snapshot) };
    let _ = sender.send((status, message));
}

#[test]
fn contract_a_worker_that_dies_still_completes_queued_commands() {
    let path = package();
    let (start, started) = mpsc::channel::<()>();
    let document = Document::open_with(
        &path,
        Box::new(move |_| {
            let _ = started.recv();
            panic!("the worker dies with a command queued");
        }),
    )
    .unwrap();
    let document = Box::into_raw(Box::new(document));
    let (sender, incoming) = mpsc::channel::<(c_int, Option<String>)>();
    let json = r#"{"type":"track.add","name":"queued"}"#;
    let code = unsafe {
        echode_document_submit(
            document,
            json.as_ptr(),
            json.len(),
            Box::into_raw(Box::new(sender)).cast(),
            report_status,
        )
    };
    assert_eq!(code, 0);
    start.send(()).unwrap();
    let (status, message) = incoming
        .recv_timeout(WAIT)
        .expect("a queued command must not lose its completion");
    assert_eq!(status, ECHODE_ERR);
    assert_eq!(message.as_deref(), Some("document thread stopped"));
    unsafe { echode_document_close(document) };
    let _ = std::fs::remove_dir_all(path);
}
