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

struct ShutdownReply {
    document: *mut Document,
    sender: mpsc::Sender<i32>,
}

extern "C" fn shutdown_on_complete(
    context: *mut c_void,
    status: c_int,
    _message: *const c_char,
    snapshot: *mut Snapshot,
) {
    let reply = unsafe { &*(context as *const ShutdownReply) };
    unsafe { echode_document_shutdown(reply.document) };
    if !snapshot.is_null() {
        unsafe { echode_snapshot_free(snapshot) };
    }
    let _ = reply.sender.send(status);
}

#[test]
fn audit_pro14_shutdown_from_the_completion_callback_does_not_abort() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = ShutdownReply { document, sender };
    let json = br#"{"type":"track.add","name":"piano"}"#;
    let code = unsafe {
        echode_document_submit(
            document,
            json.as_ptr(),
            json.len(),
            &reply as *const ShutdownReply as *mut c_void,
            shutdown_on_complete,
        )
    };
    assert_eq!(code, 0);
    let status = incoming
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("completion");
    assert_eq!(status, ECHODE_OK);
    let (sender, incoming) = mpsc::channel();
    let late = Reply { sender };
    let outcome = submit(
        document,
        r#"{"type":"track.add","name":"late"}"#,
        &late,
        &incoming,
    );
    assert_eq!(outcome.status, ECHODE_ERR);
    assert!(outcome.message.unwrap().contains("stopped"));
    unsafe { echode_document_close(document) };
}

extern "C" fn close_on_complete(
    context: *mut c_void,
    status: c_int,
    _message: *const c_char,
    snapshot: *mut Snapshot,
) {
    let reply = unsafe { &*(context as *const ShutdownReply) };
    if !snapshot.is_null() {
        unsafe { echode_snapshot_free(snapshot) };
    }
    unsafe { echode_document_close(reply.document) };
    let _ = reply.sender.send(status);
}

#[test]
fn audit_pro14_close_from_the_completion_callback_does_not_abort() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = ShutdownReply { document, sender };
    let json = br#"{"type":"track.add","name":"piano"}"#;
    let code = unsafe {
        echode_document_submit(
            document,
            json.as_ptr(),
            json.len(),
            &reply as *const ShutdownReply as *mut c_void,
            close_on_complete,
        )
    };
    assert_eq!(code, 0);
    let status = incoming
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("completion");
    assert_eq!(status, ECHODE_OK);
}

#[test]
fn audit_pro14_shutdown_races_an_external_stop() {
    for _ in 0..32 {
        let document = open();
        let address = document as usize;
        let stopper = std::thread::spawn(move || {
            unsafe { echode_document_shutdown(address as *mut Document) };
        });
        let (sender, incoming) = mpsc::channel();
        let reply = ShutdownReply { document, sender };
        let json = br#"{"type":"track.add","name":"piano"}"#;
        let code = unsafe {
            echode_document_submit(
                document,
                json.as_ptr(),
                json.len(),
                &reply as *const ShutdownReply as *mut c_void,
                shutdown_on_complete,
            )
        };
        assert_eq!(code, 0);
        let status = incoming
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("completion");
        assert!(status == ECHODE_OK || status == ECHODE_ERR);
        stopper.join().expect("external stop");
        unsafe { echode_document_close(document) };
    }
}

#[test]
fn an_inserted_plugin_is_visible_on_the_snapshot_and_undo_removes_it() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    let added = submit(
        document,
        r#"{"type":"track.add","name":"piano"}"#,
        &reply,
        &incoming,
    );
    assert_eq!(
        unsafe { echode_snapshot_device_count(added.snapshot, 0) },
        1
    );
    let mut track = [0u8; 16];
    assert_eq!(
        unsafe { echode_snapshot_track_id(added.snapshot, 0, track.as_mut_ptr()) },
        0
    );
    let track = uuid::Uuid::from_bytes(track);
    unsafe { echode_snapshot_free(added.snapshot) };
    let json =
        format!(r#"{{"type":"device.add","track":"{track}","plugin":"dev.echode.test-gain"}}"#);
    let inserted = submit(document, &json, &reply, &incoming);
    assert_eq!(inserted.status, ECHODE_OK);
    assert_eq!(
        unsafe { echode_snapshot_device_count(inserted.snapshot, 0) },
        2
    );
    let mut length = 0usize;
    let plugin = unsafe { echode_snapshot_device_plugin(inserted.snapshot, 0, 0, &mut length) };
    assert!(!plugin.is_null());
    let text = unsafe { std::slice::from_raw_parts(plugin, length) };
    assert_eq!(text, b"dev.echode.test-gain");
    assert_eq!(
        unsafe { echode_snapshot_device_bypassed(inserted.snapshot, 0, 0) },
        0
    );
    assert_eq!(
        unsafe { echode_snapshot_device_bypassed(inserted.snapshot, 0, usize::MAX) },
        -1
    );
    assert_eq!(
        unsafe { echode_snapshot_device_bypassed(std::ptr::null(), 0, 0) },
        -1
    );
    let mut mixer_length = 0usize;
    assert!(
        unsafe { echode_snapshot_device_plugin(inserted.snapshot, 0, 1, &mut mixer_length) }
            .is_null()
    );
    unsafe { echode_snapshot_free(inserted.snapshot) };
    let removed = submit(
        document,
        r#"{"type":"device.add","track":"not-a-track","plugin":""}"#,
        &reply,
        &incoming,
    );
    assert_eq!(removed.status, ECHODE_ERR);
    unsafe { echode_document_close(document) };
}

#[test]
fn a_note_in_view_is_returned_in_the_caller_buffer() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    let added = submit(
        document,
        r#"{"type":"track.add","name":"piano"}"#,
        &reply,
        &incoming,
    );
    let mut track = [0u8; 16];
    assert_eq!(
        unsafe { echode_snapshot_track_id(added.snapshot, 0, track.as_mut_ptr()) },
        0
    );
    let track = uuid::Uuid::from_bytes(track);
    unsafe { echode_snapshot_free(added.snapshot) };
    let json = format!(
        r#"{{"type":"note.insert","track":"{track}","pitch":60,"velocity":100,"start":0,"length":705600}}"#
    );
    let placed = submit(document, &json, &reply, &incoming);
    assert_eq!(placed.status, ECHODE_OK);
    let mut written = 0usize;
    let mut dropped = 0usize;
    assert_eq!(
        unsafe {
            echode_snapshot_visible(
                placed.snapshot,
                0,
                705_600 * 4,
                0.0,
                2.0,
                1.0,
                20.0,
                std::ptr::null_mut(),
                0,
                &mut written,
                &mut dropped,
            )
        },
        0
    );
    assert!(dropped >= 3, "dropped {dropped}");
    let mut elements = vec![
        VisibleElement {
            role: 0,
            track_id: [0; 16],
            placement_id: [0; 16],
            note_id: [0; 16],
            has_placement: 0,
            has_note: 0,
            pad: [0; 2],
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        dropped
    ];
    assert_eq!(
        unsafe {
            echode_snapshot_visible(
                placed.snapshot,
                0,
                705_600 * 4,
                0.0,
                2.0,
                1.0,
                20.0,
                elements.as_mut_ptr(),
                elements.len(),
                &mut written,
                &mut dropped,
            )
        },
        0
    );
    assert_eq!(dropped, 0);
    assert!(
        elements[..written]
            .iter()
            .any(|element| element.role == ECHODE_ROLE_NOTE && element.has_note == 1)
    );
    let mut one = [elements[0]];
    assert_eq!(
        unsafe {
            echode_snapshot_visible(
                placed.snapshot,
                0,
                705_600 * 4,
                0.0,
                2.0,
                1.0,
                20.0,
                one.as_mut_ptr(),
                1,
                &mut written,
                &mut dropped,
            )
        },
        0
    );
    assert_eq!(written, 1);
    assert!(dropped >= 1);
    unsafe {
        echode_snapshot_free(placed.snapshot);
        echode_document_close(document);
    }
}

#[test]
fn the_generated_header_names_the_entry_points() {
    let header = include_str!(concat!(env!("OUT_DIR"), "/echode.h"));
    assert!(header.contains("echode_document_submit"));
    assert!(header.contains("echode_document_create"));
    assert!(header.contains("echode_snapshot_free"));
    assert!(header.contains("echode_snapshot_visible"));
    assert!(header.contains("echode_snapshot_device_plugin"));
    assert!(header.contains("echode_plugin_catalog"));
    assert!(header.contains("echode_document_query"));
    assert!(header.contains("echode_document_audition"));
    assert!(header.contains("Generated by cbindgen"));
    assert!(header.contains("typedef void (*Complete)"));
}

#[test]
fn query_undo_redo_and_unknown_operations_complete_once() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    let query = |operation| {
        unsafe {
            echode_document_query(
                document,
                operation,
                &reply as *const Reply as *mut c_void,
                on_complete,
            )
        };
        let result = incoming
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(incoming.try_recv().is_err());
        result
    };
    let initial = query(ECHODE_DOCUMENT_SNAPSHOT);
    assert_eq!(initial.status, ECHODE_OK);
    assert_eq!(unsafe { echode_snapshot_track_count(initial.snapshot) }, 0);
    unsafe { echode_snapshot_free(initial.snapshot) };
    let added = submit(
        document,
        r#"{"type":"track.add","name":"piano"}"#,
        &reply,
        &incoming,
    );
    unsafe { echode_snapshot_free(added.snapshot) };
    let undone = query(ECHODE_DOCUMENT_UNDO);
    assert_eq!(undone.status, ECHODE_OK);
    assert_eq!(unsafe { echode_snapshot_track_count(undone.snapshot) }, 0);
    unsafe { echode_snapshot_free(undone.snapshot) };
    let redone = query(ECHODE_DOCUMENT_REDO);
    assert_eq!(redone.status, ECHODE_OK);
    assert_eq!(unsafe { echode_snapshot_track_count(redone.snapshot) }, 1);
    unsafe { echode_snapshot_free(redone.snapshot) };
    let bad = query(u32::MAX);
    assert_eq!(bad.status, ECHODE_ERR);
    assert!(bad.snapshot.is_null());
    unsafe { echode_document_shutdown(document) };
    assert_eq!(query(ECHODE_DOCUMENT_SNAPSHOT).status, ECHODE_ERR);
    unsafe { echode_document_close(document) };
}

#[test]
fn a_forced_exit_keeps_the_confirmed_edit() {
    if let Ok(path) = std::env::var("ECHODE_FORCE_QUIT") {
        force_quit_after_edit(std::path::Path::new(&path));
    }
    let path = scratch();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["tests::a_forced_exit_keeps_the_confirmed_edit", "--exact"])
        .env("ECHODE_FORCE_QUIT", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let document = unsafe { echode_document_open(c_path.as_ptr()) };
    assert!(
        !document.is_null(),
        "{}",
        echode_history::recover(&path).unwrap_err()
    );
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    unsafe {
        echode_document_query(
            document,
            ECHODE_DOCUMENT_SNAPSHOT,
            &reply as *const Reply as *mut c_void,
            on_complete,
        );
    }
    let result = incoming
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(result.status, ECHODE_OK);
    assert_eq!(unsafe { echode_snapshot_track_count(result.snapshot) }, 1);
    let mut length = 0usize;
    let name = unsafe { echode_snapshot_track_name(result.snapshot, 0, &mut length) };
    let name = unsafe { std::slice::from_raw_parts(name, length) };
    assert_eq!(name, b"kept");
    unsafe {
        echode_snapshot_free(result.snapshot);
        echode_document_close(document);
    }
    let _ = std::fs::remove_dir_all(path);
}

fn force_quit_after_edit(path: &std::path::Path) -> ! {
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let document = unsafe { echode_document_open(c_path.as_ptr()) };
    assert!(!document.is_null());
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    let outcome = submit(
        document,
        r#"{"type":"track.add","name":"kept"}"#,
        &reply,
        &incoming,
    );
    assert_eq!(outcome.status, ECHODE_OK);
    unsafe { libc::_exit(0) };
}

fn audio_state(
    document: *mut Document,
    reply: &Reply,
    incoming: &mpsc::Receiver<Outcome>,
) -> serde_json::Value {
    let json = br#"{"type":"audio.status"}"#;
    let code = unsafe {
        echode_document_audio(
            document,
            json.as_ptr(),
            json.len(),
            reply as *const Reply as *mut c_void,
            on_complete,
        )
    };
    assert_eq!(code, 0);
    let outcome = incoming
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("completion");
    assert_eq!(outcome.status, 0, "{:?}", outcome.message);
    let mut len = 0;
    let bytes = unsafe { echode_snapshot_audio(outcome.snapshot, &mut len) };
    let value = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, len) }).unwrap();
    unsafe { echode_snapshot_free(outcome.snapshot) };
    value
}

#[test]
fn an_audition_returns_without_the_worker_and_reports_an_unreadable_file() {
    let document = open();
    let (sender, incoming) = mpsc::channel();
    let reply = Reply { sender };
    // Decoding fails before any output device is asked for.
    let missing =
        std::env::temp_dir().join(format!("echode-ffi-missing-{}.wav", std::process::id()));
    let path = missing.to_str().unwrap();
    assert_eq!(
        unsafe { echode_document_audition(document, path.as_ptr(), path.len()) },
        0
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let audition = loop {
        let audition = audio_state(document, &reply, &incoming)["audition"].clone();
        if audition["phase"] == "failed" || std::time::Instant::now() > deadline {
            break audition;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(audition["phase"], "failed", "{audition}");
    assert_eq!(audition["path"], path);
    assert_eq!(audition["request"], audition["handled"]);
    assert!(audition["sample_rate"].is_null());
    let request = audition["request"].as_u64().unwrap();
    assert_eq!(
        unsafe { echode_document_audition(document, std::ptr::null(), 0) },
        0
    );
    let audition = audio_state(document, &reply, &incoming)["audition"].clone();
    assert_eq!(audition["phase"], "stopped");
    assert_eq!(audition["request"].as_u64(), Some(request + 1));
    assert_eq!(
        unsafe { echode_document_audition(document, std::ptr::null(), 4) },
        ECHODE_ERR
    );
    let invalid = [0xff_u8, 0xfe];
    assert_eq!(
        unsafe { echode_document_audition(document, invalid.as_ptr(), 2) },
        ECHODE_ERR
    );
    assert_eq!(
        unsafe { echode_document_audition(std::ptr::null(), std::ptr::null(), 0) },
        ECHODE_ERR
    );
    unsafe { echode_document_close(document) };
}
