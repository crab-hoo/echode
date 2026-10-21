//! C ABI for the app shell.
//!
//! A command is queued and the caller returns. The document thread applies it,
//! then the completion callback runs once, without a lock held. The spike in
//! `spikes/bridge` is not used here.

mod activity;
mod catalog;
mod controls;
mod device;
mod piano;
mod projection;
mod queue;
pub use controls::{
    ClipControls, MixerControls, echode_document_track_peak, echode_snapshot_clip_controls,
    echode_snapshot_flicks, echode_snapshot_mixer_controls,
};
pub use projection::{TrackTreeInfo, echode_snapshot_project_arrange, echode_snapshot_track_tree};

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

/// Queues a history list or restore on the document thread. The completed
/// snapshot carries the revision list. Restoring records a new version.
///
/// # Safety
/// `document` is null or open. `json` is null only when `json_len` is zero.
/// The callback owns `context` until completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_history(
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
            c"history command is null or too long".as_ptr(),
            std::ptr::null_mut(),
        );
        return ECHODE_ERR;
    }
    let bytes = if json_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_len) }
    };
    document.enqueue(queue::Operation::History(bytes.to_vec()), context, complete);
    ECHODE_OK
}

/// Queues a device command (`device.parameters`, `device.parameter.set`,
/// `device.parameter.commit`, `device.gui.show` or `device.gui.close`). Only a
/// commit that records new plugin bytes enters Undo. The completed snapshot
/// carries the JSON report.
///
/// # Safety
/// `document` is null or open. `json` is null only when `json_len` is zero.
/// The callback owns `context` until completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_device(
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
            c"device command is null or too long".as_ptr(),
            std::ptr::null_mut(),
        );
        return ECHODE_ERR;
    }
    let bytes = if json_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(json, json_len) }
    };
    document.enqueue(queue::Operation::Device(bytes.to_vec()), context, complete);
    ECHODE_OK
}

/// UTF-8 JSON report from the snapshot that completed a device command.
/// Other snapshots carry `{}`. The pointer is valid until the snapshot is freed.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device(
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
    let bytes = snapshot.device();
    *len = bytes.len();
    bytes.as_ptr()
}

/// Waits up to `timeout_ms` for plugin activity newer than `after` and writes
/// it as UTF-8 JSON `{"next", "closed", "entries"}`: window notices and the
/// values and gestures each plugin made on its own. Entries up to `after` are
/// dropped, because the caller already has them; pass `next` on the next call.
///
/// Returns 0 when `out` holds the page. Returns 1 when `capacity` is too
/// small and `written` is the size needed; nothing newer is dropped, so the
/// next call returns at once. Returns -1 for a null document or `written`.
/// Never waits for the document thread.
///
/// # Safety
/// `document` is null or open. `written` is writable. `out` is null or has
/// room for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_plugin_activity(
    document: *const Document,
    after: u64,
    timeout_ms: u32,
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> c_int {
    let (Some(document), Some(written)) =
        (unsafe { document.as_ref() }, unsafe { written.as_mut() })
    else {
        return -1;
    };
    let json = document.plugin_activity(
        after,
        std::time::Duration::from_millis(u64::from(timeout_ms)),
    );
    *written = json.len();
    if out.is_null() || capacity < json.len() {
        return 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(json.as_ptr(), out, json.len());
    }
    0
}

/// An audition request that came from neither a click nor a key.
pub const ECHODE_AUDITION_INPUT_OTHER: u32 = 0;
/// An audition request made by a pointer click.
pub const ECHODE_AUDITION_INPUT_CLICK: u32 = 1;
/// An audition request made by a key press.
pub const ECHODE_AUDITION_INPUT_KEY: u32 = 2;

/// Plays the audio file at the UTF-8 `path` once over the project, from its
/// first sample and without the project's FX or faders, replacing any earlier
/// audition. A null or empty `path` fades the audition out. Returns at once:
/// decoding and the audition output run on their own thread, not the document
/// worker. The next snapshot's audio state reports the newest request.
///
/// `input` is one of the `ECHODE_AUDITION_INPUT_*` values and
/// `input_delay_us` the microseconds from that click or key to this call.
/// Both are only recorded in the audition's history; the delay is ignored
/// for `ECHODE_AUDITION_INPUT_OTHER`.
///
/// # Safety
/// `document` is null or open. `path` is null only when `path_len` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_audition(
    document: *const Document,
    path: *const u8,
    path_len: usize,
    input: u32,
    input_delay_us: u64,
) -> c_int {
    let Some(document) = (unsafe { document.as_ref() }) else {
        return ECHODE_ERR;
    };
    if path_len > isize::MAX as usize || (path.is_null() && path_len != 0) {
        return ECHODE_ERR;
    }
    let input = match input {
        ECHODE_AUDITION_INPUT_OTHER => echode_audio_io::AuditionInput::Other,
        ECHODE_AUDITION_INPUT_CLICK => echode_audio_io::AuditionInput::Click,
        ECHODE_AUDITION_INPUT_KEY => echode_audio_io::AuditionInput::Key,
        _ => return ECHODE_ERR,
    };
    let origin = echode_audio_io::AuditionOrigin {
        input,
        delay_us: (input != echode_audio_io::AuditionInput::Other).then_some(input_delay_us),
    };
    if path_len == 0 {
        document.audition(None, origin);
        return ECHODE_OK;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, path_len) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ECHODE_ERR;
    };
    document.audition(Some(std::path::PathBuf::from(text)), origin);
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

/// UTF-8 JSON `{revisions, changed}` from the snapshot that completed a history
/// command. Other snapshots carry an empty list. The pointer is valid until
/// the snapshot is freed.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_history(
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
    let bytes = snapshot.history();
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

/// Returns 1 for bypassed, 0 for active, or -1 for an invalid snapshot/index.
///
/// # Safety
/// `snapshot` is null or owned for the whole read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_bypassed(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
) -> c_int {
    snapshot_ref(snapshot)
        .and_then(|snapshot| snapshot.device_bypassed(track, device))
        .map_or(-1, c_int::from)
}

/// Returns 1 for offline, 0 for online, or -1 for an invalid snapshot/index.
///
/// # Safety
/// `snapshot` is null or owned for the whole read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_offline(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
) -> c_int {
    snapshot_ref(snapshot)
        .and_then(|snapshot| snapshot.device_offline(track, device))
        .map_or(-1, c_int::from)
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

/// Borrows the ascending CLAP parameter ids pinned on a device. An invalid
/// snapshot or index returns null with a zero count; no pins return a count of 0.
///
/// # Safety
/// `count` is writable. `snapshot` is null or owned for the whole read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_pins(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    count: *mut usize,
) -> *const u32 {
    if count.is_null() {
        return std::ptr::null();
    }
    unsafe {
        *count = 0;
    }
    let Some(pins) =
        snapshot_ref(snapshot).and_then(|snapshot| snapshot.device_pins(track, device))
    else {
        return std::ptr::null();
    };
    unsafe {
        *count = pins.len();
    }
    pins.as_ptr()
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

/// Starts the background CLAP folder watch. A second call does nothing.
///
/// Returns 0 when the watch is running. Returns `-1` when the helper cannot
/// be started. The helper process loads each bundle.
///
/// # Safety
/// There are no pointer arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_plugin_watch_start() -> c_int {
    if catalog::start_watch().is_ok() {
        0
    } else {
        -1
    }
}

/// Waits until the published catalog revision is not `revision`.
///
/// Returns the revision after the wait. When the watch is not running, returns
/// `revision` without waiting. `timeout_ms` is how long to block.
///
/// # Safety
/// There are no pointer arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_plugin_watch_wait(revision: u64, timeout_ms: u32) -> u64 {
    catalog::wait_watch(revision, timeout_ms)
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

/// Writes min and max for each visible column of one audio placement.
///
/// `placement` is 16 UUID bytes. The tick window is the clip rectangle's
/// time. A note placement or a missing file writes nothing.
///
/// # Safety
/// `snapshot` is null or still owned. `mins` and `maxs` each hold `capacity`
/// floats for the whole call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_waveform(
    snapshot: *const Snapshot,
    placement: *const u8,
    start_tick: i64,
    end_tick: i64,
    mins: *mut f32,
    maxs: *mut f32,
    capacity: usize,
) -> usize {
    if placement.is_null() || mins.is_null() || maxs.is_null() || capacity == 0 {
        return 0;
    }
    let Some(snapshot) = snapshot_ref(snapshot) else {
        return 0;
    };
    let bytes = unsafe { *(placement as *const [u8; 16]) };
    let placement = echode_model::Id::from_raw(u128::from_be_bytes(bytes));
    let mut columns = vec![echode_history::PeakColumn { min: 0.0, max: 0.0 }; capacity];
    let written = snapshot.waveform(placement, start_tick, end_tick, &mut columns);
    let mins = unsafe { std::slice::from_raw_parts_mut(mins, written) };
    let maxs = unsafe { std::slice::from_raw_parts_mut(maxs, written) };
    for (index, column) in columns.iter().take(written).enumerate() {
        mins[index] = column.min;
        maxs[index] = column.max;
    }
    written
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
mod tests;

#[cfg(test)]
mod opening_tests;

// Pure snapshot tests also run under Miri. No package IO, UUID randomness, or OS
// threads are needed to check borrowed names, retained ownership and capacities.
#[cfg(test)]
mod snapshot_ownership;
