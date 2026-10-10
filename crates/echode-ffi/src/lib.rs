//! C ABI for the app shell.
//!
//! A command is queued and the caller returns. The document thread applies it,
//! then the completion callback runs once, without a lock held. The spike in
//! `spikes/bridge` is not used here.

mod catalog;
mod piano;
mod queue;

pub use queue::{Document, Snapshot};

/// A coherent copy of the live playhead. This query is safe on a draw thread.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportPosition {
    pub tick: i64,
    pub playing: u32,
    pub pad: u32,
}

/// Queues a session command. Transport/output settings do not enter Undo or
/// the durable project journal.
///
/// # Safety
/// Same pointer and callback ownership contract as echode_document_submit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_audio(
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
        return ECHODE_ERR;
    };
    if json_len > isize::MAX as usize || (json.is_null() && json_len != 0) {
        complete(
            context,
            ECHODE_ERR,
            c"audio command is null or too long".as_ptr(),
            std::ptr::null_mut(),
        );
        return ECHODE_ERR;
    }
    let bytes = if json_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_len) }
    };
    document.enqueue(queue::Operation::Audio(bytes.to_vec()), context, complete);
    ECHODE_OK
}

/// Reads the live playhead without submitting a command or taking a lock.
///
/// # Safety
/// document is still owned; out points to writable TransportPosition storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_position(
    document: *const Document,
    out: *mut TransportPosition,
) -> c_int {
    let (Some(document), Some(out)) = (unsafe { document.as_ref() }, unsafe { out.as_mut() })
    else {
        return ECHODE_ERR;
    };
    *out = document.position();
    ECHODE_OK
}

/// Borrowed UTF-8 JSON session state for this immutable snapshot.
///
/// # Safety
/// snapshot is owned; len points to writable size_t storage. Bytes are valid
/// only until this snapshot is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_audio(
    snapshot: *const Snapshot,
    len: *mut usize,
) -> *const u8 {
    let Some(len) = (unsafe { len.as_mut() }) else {
        return std::ptr::null();
    };
    *len = 0;
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return std::ptr::null();
    };
    let bytes = snapshot.audio();
    *len = bytes.len();
    bytes.as_ptr()
}

use std::ffi::{CStr, c_char, c_void};
use std::os::raw::c_int;

use echode_history::write_package;
use echode_model::Project;

/// A track lane.
pub const ECHODE_ROLE_TRACK: u32 = 0;
/// A clip placement.
pub const ECHODE_ROLE_CLIP: u32 = 1;
/// A note heard through a placement.
pub const ECHODE_ROLE_NOTE: u32 = 2;

/// One visible arrange object. Ids use the UUID byte order of the model.
/// Absent placement and note ids are zero, and the corresponding flag is 0.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VisibleElement {
    pub role: u32,
    pub track_id: [u8; 16],
    pub placement_id: [u8; 16],
    pub note_id: [u8; 16],
    pub has_placement: u8,
    pub has_note: u8,
    pub pad: [u8; 2],
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const _: () = assert!(std::mem::size_of::<VisibleElement>() == 72);

/// A piano note's full timeline geometry and its visible pixel rectangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PianoNoteElement {
    pub track_id: [u8; 16],
    pub placement_id: [u8; 16],
    pub note_id: [u8; 16],
    pub start: i64,
    pub length: i64,
    pub window_start: i64,
    pub window_end: i64,
    pub pitch: u32,
    pub velocity: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const _: () = assert!(std::mem::size_of::<PianoNoteElement>() == 104);

/// The command was applied. `snapshot` is owned by the caller.
pub const ECHODE_OK: c_int = 0;
/// The command was rejected, or the document thread has stopped.
pub const ECHODE_ERR: c_int = 1;

pub const ECHODE_DOCUMENT_SNAPSHOT: u32 = 0;
pub const ECHODE_DOCUMENT_UNDO: u32 = 1;
pub const ECHODE_DOCUMENT_REDO: u32 = 2;

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
    // Creation never replaces an existing package or another user's directory.
    if path.exists() {
        return std::ptr::null_mut();
    }
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

/// Queues a snapshot, Undo, or Redo on the same queue as edits. Completes once,
/// including unknown operations and stopped documents, without holding a lock.
///
/// # Safety
/// `document` is null or open. The callback owns `context` until completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_query(
    document: *mut Document,
    operation: u32,
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
    let operation = match operation {
        ECHODE_DOCUMENT_SNAPSHOT => queue::Operation::Snapshot,
        ECHODE_DOCUMENT_UNDO => queue::Operation::Undo,
        ECHODE_DOCUMENT_REDO => queue::Operation::Redo,
        _ => queue::Operation::Invalid,
    };
    document.enqueue(operation, context, complete);
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
pub unsafe extern "C" fn echode_snapshot_track_count(snapshot: *const Snapshot) -> usize {
    snapshot_ref(snapshot)
        .map(Snapshot::track_count)
        .unwrap_or(0)
}

/// Writes the UUID bytes of track `index`. Returns 0, or `-1` when it is missing.
///
/// # Safety
/// `out` points at 16 writable bytes. `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_id(
    snapshot: *const Snapshot,
    index: usize,
    out: *mut u8,
) -> c_int {
    let Some(out) = out_bytes(out) else {
        return -1;
    };
    let Some(snapshot) = snapshot_ref(snapshot) else {
        return -1;
    };
    if snapshot.write_track_id(index, out) {
        0
    } else {
        -1
    }
}

/// Borrows the UTF-8 name of track `index`, without a trailing NUL. Missing
/// tracks return null and length zero. No model storage is copied or mutated.
///
/// # Safety
/// `length` is writable. `snapshot` is null or owned; keep it alive for the
/// entire read of the returned `length` bytes, and never write through them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_name(
    snapshot: *const Snapshot,
    index: usize,
    length: *mut usize,
) -> *const u8 {
    if length.is_null() {
        return std::ptr::null();
    }
    unsafe {
        *length = 0;
    }
    let Some(name) = snapshot_ref(snapshot).and_then(|snapshot| snapshot.track_name(index)) else {
        return std::ptr::null();
    };
    unsafe {
        *length = name.len();
    }
    name.as_ptr()
}

/// How many devices the track has, including its mixer. A missing track is zero.
///
/// # Safety
/// `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_count(
    snapshot: *const Snapshot,
    track: usize,
) -> usize {
    snapshot_ref(snapshot)
        .map(|snapshot| snapshot.device_count(track))
        .unwrap_or(0)
}

/// Writes the UUID bytes of a device. Returns 0, or `-1` when it is missing.
///
/// # Safety
/// `out` points at 16 writable bytes. `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_id(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    out: *mut u8,
) -> c_int {
    let Some(out) = out_bytes(out) else {
        return -1;
    };
    let Some(snapshot) = snapshot_ref(snapshot) else {
        return -1;
    };
    if snapshot.write_device_id(track, device, out) {
        0
    } else {
        -1
    }
}

/// Borrows the CLAP id of a device. The mixer has none and returns null.
///
/// # Safety
/// `length` is writable. `snapshot` is null or owned for the whole read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_plugin(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    length: *mut usize,
) -> *const u8 {
    if length.is_null() {
        return std::ptr::null();
    }
    unsafe {
        *length = 0;
    }
    let Some(plugin) =
        snapshot_ref(snapshot).and_then(|snapshot| snapshot.device_plugin(track, device))
    else {
        return std::ptr::null();
    };
    unsafe {
        *length = plugin.len();
    }
    plugin.as_ptr()
}

/// Writes the installed CLAP catalog as UTF-8 JSON.
///
/// Returns 0 when `out` holds the document. Returns 1 when `capacity` is too
/// small and `written` is the required size. Returns `-1` when the helper
/// cannot be started. The helper process loads each bundle.
///
/// # Safety
/// `written` is writable. `out` is null or has room for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_plugin_catalog(
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> c_int {
    if written.is_null() {
        return -1;
    }
    unsafe {
        *written = 0;
    }
    let Ok(json) = catalog::plugins_json() else {
        return -1;
    };
    unsafe {
        *written = json.len();
    }
    if out.is_null() || capacity < json.len() {
        return 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(json.as_ptr(), out, json.len());
    }
    0
}

/// Writes arrange elements that intersect the half-open view into `out`.
///
/// `written` and `dropped` are required. A null `out` with `capacity` 0 counts
/// every element as dropped and does not allocate.
///
/// # Safety
/// `out` has room for `capacity` elements. `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_visible(
    snapshot: *const Snapshot,
    origin_tick: i64,
    width_ticks: i64,
    origin_track: f64,
    height_tracks: f64,
    pixels_per_tick: f64,
    pixels_per_track: f64,
    out: *mut VisibleElement,
    capacity: usize,
    written: *mut usize,
    dropped: *mut usize,
) -> c_int {
    if written.is_null() || dropped.is_null() {
        return -1;
    }
    unsafe {
        *written = 0;
        *dropped = 0;
    }
    if out.is_null() && capacity != 0 {
        return -1;
    }
    let Some(snapshot) = snapshot_ref(snapshot) else {
        unsafe {
            *written = 0;
            *dropped = 0;
        }
        return -1;
    };
    let view = echode_draw::ArrangeView {
        origin_tick: echode_model::Tick::new(origin_tick),
        width_ticks,
        origin_track,
        height_tracks,
        pixels_per_tick,
        pixels_per_track,
    };
    let slot = if out.is_null() || capacity == 0 {
        &mut [][..]
    } else {
        unsafe { std::slice::from_raw_parts_mut(out, capacity) }
    };
    let (filled, overflow) = snapshot.write_visible(view, slot);
    unsafe {
        *written = filled;
        *dropped = overflow;
    }
    0
}

fn out_bytes<'a>(out: *mut u8) -> Option<&'a mut [u8; 16]> {
    if out.is_null() {
        None
    } else {
        Some(unsafe { &mut *(out as *mut [u8; 16]) })
    }
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
}

// Pure snapshot tests also run under Miri. No package IO, UUID randomness, or OS
// threads are needed to check borrowed names, retained ownership and capacities.
#[cfg(test)]
mod snapshot_ownership {
    use super::*;
    use echode_model::{Id, MixerIds};
    use std::sync::Arc;

    fn snapshot() -> *mut Snapshot {
        let project = Project::new()
            .add_track_with_id(Id::from_raw(1), "鍵盤 🎹", MixerIds::from_base(16))
            .unwrap();
        Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)))
    }

    #[test]
    fn retained_snapshot_keeps_borrowed_utf8_and_uuid_alive() {
        let original = snapshot();
        let retained = unsafe { echode_snapshot_retain(original) };
        let mut length = 0;
        let name = unsafe { echode_snapshot_track_name(retained, 0, &mut length) };
        unsafe { echode_snapshot_free(original) };
        assert_eq!(
            unsafe { std::slice::from_raw_parts(name, length) },
            "鍵盤 🎹".as_bytes()
        );
        let mut bytes = [0; 16];
        assert_eq!(
            unsafe { echode_snapshot_track_id(retained, 0, bytes.as_mut_ptr()) },
            0
        );
        assert_eq!(uuid::Uuid::from_bytes(bytes).as_u128(), 1);
        assert!(unsafe { echode_snapshot_track_name(retained, 1, &mut length) }.is_null());
        assert_eq!(length, 0);
        unsafe { echode_snapshot_free(retained) };
    }

    #[test]
    fn retained_audio_json_stays_owned_and_null_queries_clear_the_length() {
        let original = snapshot();
        let retained = unsafe { echode_snapshot_retain(original) };
        let mut length = 0usize;
        let pointer = unsafe { echode_snapshot_audio(retained, &mut length) };
        unsafe { echode_snapshot_free(original) };
        let state: serde_json::Value =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(pointer, length) }).unwrap();
        assert_eq!(state["playing"], false);
        assert_eq!(state["tick"], 0);
        assert_eq!(state["devices"], serde_json::json!([]));
        assert!(unsafe { echode_snapshot_audio(std::ptr::null(), &mut length) }.is_null());
        assert_eq!(length, 0);
        assert!(unsafe { echode_snapshot_audio(retained, std::ptr::null_mut()) }.is_null());
        unsafe { echode_snapshot_free(retained) };
    }

    #[test]
    fn visible_capacity_and_null_contract_leave_the_canary_intact() {
        let snapshot = snapshot();
        let empty = VisibleElement {
            role: u32::MAX,
            track_id: [0; 16],
            placement_id: [0; 16],
            note_id: [0; 16],
            has_placement: 0,
            has_note: 0,
            pad: [0; 2],
            x: 0.,
            y: 0.,
            w: 0.,
            h: 0.,
        };
        let mut elements = [empty; 2]; // One writable element followed by a canary, for this test only.
        let mut written = 0;
        let mut dropped = 0;
        let mut query = |out, capacity, origin| unsafe {
            echode_snapshot_visible(
                snapshot,
                0,
                705_600,
                origin,
                1.,
                1.,
                64.,
                out,
                capacity,
                &mut written,
                &mut dropped,
            )
        };
        assert_eq!(query(std::ptr::null_mut(), 0, 0.), 0);
        assert_eq!(query(elements.as_mut_ptr(), 1, 0.), 0);
        assert_eq!(elements[0].role, ECHODE_ROLE_TRACK);
        assert_eq!(elements[1].role, u32::MAX);
        assert_eq!(query(elements.as_mut_ptr(), 1, f64::NAN), 0);
        assert_eq!(query(std::ptr::null_mut(), 1, 0.), -1);
        assert_eq!(written, 0);
        assert_eq!(dropped, 0);
        unsafe { echode_snapshot_free(snapshot) };
    }

    #[test]
    fn piano_buffers_and_navigation_keep_ids_alive_after_retain() {
        use crate::piano::{
            echode_snapshot_piano_neighbor, echode_snapshot_piano_note,
            echode_snapshot_piano_visible,
        };
        use echode_model::{NoteDraft, Tick};
        let track = Id::from_raw(1);
        let project = Project::new()
            .add_track_with_id(track, "keys", MixerIds::from_base(16))
            .unwrap()
            .insert_note_with_id(
                track,
                Id::from_raw(32),
                Id::from_raw(33),
                Id::from_raw(34),
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(705_600),
                },
            )
            .unwrap();
        let original = Box::into_raw(Box::new(Snapshot::from_project(Arc::new(project), None)));
        let retained = unsafe { echode_snapshot_retain(original) };
        unsafe { echode_snapshot_free(original) };
        let track = track.raw().to_be_bytes();
        let canary = PianoNoteElement {
            pitch: u32::MAX,
            ..PianoNoteElement::default()
        };
        let mut out = [canary; 2]; // One output and a canary for this ABI test only.
        let (mut written, mut dropped) = (0, 0);
        let mut query = |output, capacity, top| unsafe {
            echode_snapshot_piano_visible(
                retained,
                track.as_ptr(),
                0,
                705_600 * 4,
                top,
                2.,
                1. / 705_600.,
                20.,
                output,
                capacity,
                &mut written,
                &mut dropped,
            )
        };
        assert_eq!(query(std::ptr::null_mut(), 0, 60.), 0);
        assert_eq!(query(out.as_mut_ptr(), 1, 60.), 0);
        assert_eq!(out[0].pitch, 60);
        assert_eq!(out[1], canary);
        assert_eq!(query(std::ptr::null_mut(), 1, 60.), -1);
        assert_eq!((written, dropped), (0, 0));
        let mut resolved = canary;
        assert_eq!(
            unsafe {
                echode_snapshot_piano_note(
                    retained,
                    track.as_ptr(),
                    out[0].placement_id.as_ptr(),
                    out[0].note_id.as_ptr(),
                    &mut resolved,
                )
            },
            0
        );
        assert_eq!(resolved.note_id, out[0].note_id);
        assert_eq!(
            unsafe {
                echode_snapshot_piano_neighbor(
                    retained,
                    track.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &mut resolved,
                )
            },
            0
        );
        assert_eq!(resolved.note_id, out[0].note_id);
        assert_eq!(
            unsafe {
                echode_snapshot_piano_neighbor(
                    retained,
                    track.as_ptr(),
                    out[0].placement_id.as_ptr(),
                    out[0].note_id.as_ptr(),
                    0,
                    &mut resolved,
                )
            },
            -1
        );
        unsafe { echode_snapshot_free(retained) };
    }
}
