//! The first device view of a stopped document opens the output while the
//! worker keeps its other turns (ADR 0136). A device-free output holds the
//! preparation; the worker, lane, session and C entry points are the product's.

use super::*;
use echode_audio_io::HeadlessOutput;
use echode_history::write_package;
use echode_model::{Id, Project};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

struct Tagged {
    tag: &'static str,
    sender: Sender<(&'static str, i32, Option<String>, *mut Snapshot)>,
}

// SAFETY: the snapshot pointer only moves to the test thread, which frees it there; the sender is
// `Send`.
unsafe impl Send for Tagged {}

extern "C" fn tagged(
    context: *mut c_void,
    status: c_int,
    message: *const c_char,
    snapshot: *mut Snapshot,
) {
    // SAFETY: `send` leaked this box as the context and never uses it again.
    let reply = unsafe { Box::from_raw(context.cast::<Tagged>()) };
    let message = (!message.is_null()).then(|| {
        // SAFETY: a non-null message is a NUL-terminated string, valid for this call.
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    });
    let _ = reply.sender.send((reply.tag, status, message, snapshot));
}

type Completions = Receiver<(&'static str, i32, Option<String>, *mut Snapshot)>;

/// Releases held preparations even when an assertion fails, so the document
/// can close and the failure is reported instead of a hang.
struct Release(HeadlessOutput);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn package(device: Id) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("echode-ffi-opening-{}", Id::generate()));
    let (project, track) = Project::new().add_track("fx");
    let project = project
        .add_clap_device(track, device, "org.example.fx")
        .unwrap();
    write_package(&path, &project).unwrap();
    path
}

type Entry = unsafe extern "C" fn(*mut Document, *const u8, usize, *mut c_void, Complete) -> c_int;

fn send(
    entry: Entry,
    document: *mut Document,
    json: &str,
    tag: &'static str,
    sender: &Sender<(&'static str, i32, Option<String>, *mut Snapshot)>,
) {
    let context = Box::into_raw(Box::new(Tagged {
        tag,
        sender: sender.clone(),
    }));
    // SAFETY: `entry` is a queued entry point, the document is open, `json` is `json.len()`
    // readable bytes and the boxed context is handed to the completion.
    let code = unsafe { entry(document, json.as_ptr(), json.len(), context.cast(), tagged) };
    assert_eq!(code, ECHODE_OK, "{tag}");
}

/// The next completion, which must be `tag`.
fn next(incoming: &Completions, tag: &str) -> (i32, Option<String>, *mut Snapshot) {
    let (done, status, message, snapshot) = incoming
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("{tag} did not complete"));
    assert_eq!(done, tag, "completion order");
    (status, message, snapshot)
}

fn ok(incoming: &Completions, tag: &str) -> *mut Snapshot {
    let (status, message, snapshot) = next(incoming, tag);
    assert_eq!(status, ECHODE_OK, "{tag}: {message:?}");
    snapshot
}

fn audio(snapshot: *mut Snapshot) -> serde_json::Value {
    let mut len = 0;
    // SAFETY: the snapshot is owned and `len` is writable.
    let bytes = unsafe { echode_snapshot_audio(snapshot, &mut len) };
    // SAFETY: the snapshot lends `len` bytes at `bytes` until it is freed below.
    let value = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, len) }).unwrap();
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(snapshot) };
    value
}

fn until_waiting(output: &HeadlessOutput) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while output.waiting() == 0 {
        assert!(Instant::now() < deadline, "preparation never started");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn contract_a_first_device_view_waits_for_the_output_without_holding_edits() {
    let device = Id::from_raw(0xE1);
    let path = package(device);
    let output = HeadlessOutput::default();
    output.hold();
    let release = Release(output.clone());
    let document = Box::into_raw(Box::new(
        Document::open_headless(&path, output.clone()).unwrap(),
    ));
    let (sender, incoming) = mpsc::channel();
    send(
        echode_document_device,
        document,
        &format!(r#"{{"type":"device.parameters","device":"{device}"}}"#),
        "list",
        &sender,
    );
    until_waiting(&output);
    // A later request for the same device waits behind the first, in order.
    send(
        echode_document_device,
        document,
        &format!(r#"{{"type":"device.gui.close","device":"{device}"}}"#),
        "close",
        &sender,
    );
    // Edits, Undo, status and transport complete while helpers start.
    send(
        echode_document_submit,
        document,
        r#"{"type":"track.add","name":"while opening"}"#,
        "edit",
        &sender,
    );
    let edited = ok(&incoming, "edit");
    // SAFETY: the snapshot is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(edited) }, 2);
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(edited) };
    let context = Box::into_raw(Box::new(Tagged {
        tag: "undo",
        sender: sender.clone(),
    }));
    let code =
        // SAFETY: the document is open and the boxed context is handed to the completion.
        unsafe { echode_document_query(document, ECHODE_DOCUMENT_UNDO, context.cast(), tagged) };
    assert_eq!(code, ECHODE_OK);
    let undone = ok(&incoming, "undo");
    // SAFETY: the snapshot is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(undone) }, 1);
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(undone) };
    send(
        echode_document_audio,
        document,
        r#"{"type":"transport.stop"}"#,
        "stop",
        &sender,
    );
    let state = audio(ok(&incoming, "stop"));
    assert_eq!(state["opening"], true, "{state}");
    assert_eq!(state["playing"], false);
    assert!(
        incoming.try_recv().is_err(),
        "device requests wait for the output"
    );
    assert_eq!(output.waiting() + output.prepared(), 1, "one opening only");
    drop(release);
    // This plugin is not installed: the list reports that after the opening,
    // then the close completes without a helper.
    let (status, message, _) = next(&incoming, "list");
    assert_eq!(status, ECHODE_ERR);
    assert!(message.is_some_and(|message| !message.is_empty()));
    let closed = ok(&incoming, "close");
    let mut len = 0;
    // SAFETY: the snapshot is owned and `len` is writable.
    let report = unsafe { echode_snapshot_device(closed, &mut len) };
    assert_eq!(
        // SAFETY: the snapshot lends `len` bytes at `report` until it is freed below.
        unsafe { std::slice::from_raw_parts(report, len) },
        br#"{"closed":true}"#
    );
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(closed) };
    // The open output is kept: a later request does not prepare again.
    send(
        echode_document_device,
        document,
        &format!(r#"{{"type":"device.parameters","device":"{device}"}}"#),
        "again",
        &sender,
    );
    let (status, _, _) = next(&incoming, "again");
    assert_eq!(status, ECHODE_ERR);
    assert_eq!(output.prepared(), 1);
    // SAFETY: the document came from `Box::into_raw` and is not used after this.
    unsafe { echode_document_close(document) };
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn closing_while_the_output_opens_refuses_the_waiting_requests() {
    let device = Id::from_raw(0xE2);
    let path = package(device);
    let output = HeadlessOutput::default();
    output.hold();
    let release = Release(output.clone());
    let document = Box::into_raw(Box::new(
        Document::open_headless(&path, output.clone()).unwrap(),
    ));
    let (sender, incoming) = mpsc::channel();
    send(
        echode_document_device,
        document,
        &format!(r#"{{"type":"device.parameters","device":"{device}"}}"#),
        "list",
        &sender,
    );
    until_waiting(&output);
    // The preparation cannot be interrupted; closing waits for it as it
    // would for a synchronous open, after refusing the request.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(release);
    });
    // SAFETY: the document came from `Box::into_raw` and is not used after this.
    unsafe { echode_document_close(document) };
    releaser.join().unwrap();
    let (status, message, _) = next(&incoming, "list");
    assert_eq!(status, ECHODE_ERR);
    assert_eq!(message.as_deref(), Some("document thread stopped"));
    let _ = std::fs::remove_dir_all(path);
}

fn status(
    document: *mut Document,
    sender: &Sender<(&'static str, i32, Option<String>, *mut Snapshot)>,
    incoming: &Completions,
) -> serde_json::Value {
    send(
        echode_document_audio,
        document,
        r#"{"type":"audio.status"}"#,
        "status",
        sender,
    );
    audio(ok(incoming, "status"))
}

/// The app prepares the output when a project opens (ADR 0147). The request
/// returns at once, edits keep their turns, and the first play reuses it.
#[test]
fn contract_prepare_opens_the_output_without_holding_edits_and_play_reuses_it() {
    let path = package(Id::from_raw(0xE3));
    let output = HeadlessOutput::default();
    output.hold();
    let release = Release(output.clone());
    let document = Box::into_raw(Box::new(
        Document::open_headless(&path, output.clone()).unwrap(),
    ));
    let (sender, incoming) = mpsc::channel();
    send(
        echode_document_audio,
        document,
        r#"{"type":"audio.prepare"}"#,
        "prepare",
        &sender,
    );
    let state = audio(ok(&incoming, "prepare"));
    assert_eq!(state["opening"], true, "{state}");
    assert_eq!(state["playing"], false);
    until_waiting(&output);
    send(
        echode_document_submit,
        document,
        r#"{"type":"track.add","name":"while preparing"}"#,
        "edit",
        &sender,
    );
    let edited = ok(&incoming, "edit");
    // SAFETY: the snapshot is owned and not yet freed.
    assert_eq!(unsafe { echode_snapshot_track_count(edited) }, 2);
    // SAFETY: the snapshot is owned and freed once.
    unsafe { echode_snapshot_free(edited) };
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(10);
    while status(document, &sender, &incoming)["opening"] == true {
        assert!(Instant::now() < deadline, "the output never opened");
        std::thread::sleep(Duration::from_millis(1));
    }
    send(
        echode_document_audio,
        document,
        r#"{"type":"transport.play"}"#,
        "play",
        &sender,
    );
    let state = audio(ok(&incoming, "play"));
    assert_eq!(state["opening"], false, "{state}");
    assert_eq!(state["playing"], true);
    assert!(state["error"].is_null(), "{state}");
    assert_eq!(output.prepared(), 1, "play reused the prepared output");
    // SAFETY: the document came from `Box::into_raw` and is not used after this.
    unsafe { echode_document_close(document) };
    let _ = std::fs::remove_dir_all(path);
}
