//! C ABI for the app shell.
//!
//! A command is queued and the caller returns. The document thread applies it,
//! then the completion callback runs once, without a lock held. The spike in
//! `spikes/bridge` is not used here.
//!
//! Every entry point reaches the caller's memory through `abi`, so all of them
//! refuse the same arguments, like a missing one: a null pointer with a nonzero
//! length or capacity, a length beyond `isize::MAX` bytes, and an address
//! misaligned for its type (ADR 0321). Each `# Safety` section states only what
//! no check can see: that non-null memory is live, of the stated length, and
//! not shared with another writer for the call.

mod abi;
mod catalog;
mod constants;
mod controls;
mod document;
mod logging;
mod overview;
mod piano;
mod projection;
mod snapshot;
pub use constants::{ModelConstants, echode_model_constants};
pub use controls::{
    ClipControls, MixerControls, echode_document_track_peak, echode_snapshot_clip_controls,
    echode_snapshot_flicks, echode_snapshot_mixer_controls,
};
pub use projection::{TrackTreeInfo, echode_snapshot_project_arrange, echode_snapshot_track_tree};

pub use document::Document;
pub use logging::{
    ECHODE_LOG_DEBUG, ECHODE_LOG_ERROR, ECHODE_LOG_FILE_LIMIT, ECHODE_LOG_INFO, ECHODE_LOG_WARN,
    echode_log_append_lines, echode_log_flush, echode_log_install, echode_log_write,
};
pub use snapshot::Snapshot;

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
    // SAFETY: this function's contract is `queue`'s.
    unsafe {
        queue(
            document,
            json,
            json_len,
            context,
            complete,
            Operation::Audio,
            c"audio command is null or too long",
        )
    }
}

/// Queues a history list or restore on the document thread. The completed
/// snapshot carries the revision list. Restoring records a new version.
///
/// # Safety
/// `document` is null or open. `json` is null or addresses `json_len` readable bytes.
/// The callback owns `context` until completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_history(
    document: *mut Document,
    json: *const u8,
    json_len: usize,
    context: *mut c_void,
    complete: Complete,
) -> c_int {
    // SAFETY: this function's contract is `queue`'s.
    unsafe {
        queue(
            document,
            json,
            json_len,
            context,
            complete,
            Operation::History,
            c"history command is null or too long",
        )
    }
}

/// Queues a device command (`device.parameters`, `device.parameter.set`,
/// `device.parameter.commit`, `device.parameter.parse`, `device.gui.show` or
/// `device.gui.close`). Only a
/// commit that records new plugin bytes enters Undo. The completed snapshot
/// carries the JSON report.
///
/// # Safety
/// `document` is null or open. `json` is null or addresses `json_len` readable bytes.
/// The callback owns `context` until completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_device(
    document: *mut Document,
    json: *const u8,
    json_len: usize,
    context: *mut c_void,
    complete: Complete,
) -> c_int {
    // SAFETY: this function's contract is `queue`'s.
    unsafe {
        queue(
            document,
            json,
            json_len,
            context,
            complete,
            Operation::Device,
            c"device command is null or too long",
        )
    }
}

/// UTF-8 JSON report from the snapshot that completed a device command.
/// Other snapshots carry `{}`. The pointer is valid until the snapshot is freed.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device(
    snapshot: *const Snapshot,
    len: *mut usize,
) -> *const u8 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe { lend(snapshot, len, |snapshot| Some(snapshot.device())) }
}

/// Waits up to `timeout_ms` for plugin activity newer than `after` and writes
/// it as UTF-8 JSON `{"next", "closed", "entries"}`: window notices and the
/// values and gestures each plugin made on its own. Entries up to `after` are
/// dropped, because the caller already has them; pass `next` on the next call.
///
/// Returns 0 when `out` holds the page. Returns 1 when `capacity` is too
/// small and `written` is the size needed; nothing newer is dropped, so the
/// next call returns at once. Returns -1, before waiting, for a null document
/// or `written`, or a refused `out` (see `abi`). Never waits for the document
/// thread.
///
/// # Safety
/// `document` is null or open. `written` is null or writable. `out` is null or has room
/// for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_plugin_activity(
    document: *const Document,
    after: u64,
    timeout_ms: u32,
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> c_int {
    // SAFETY: the caller passes null or an open document, a writable
    // `written`, and room for `capacity` bytes at a non-null `out`.
    let (Some(document), Some(mut written), Some(out)) = (unsafe {
        (
            abi::handle(document),
            abi::place(written),
            abi::output(out, capacity),
        )
    }) else {
        return -1;
    };
    let json = document.service.plugin_activity(
        after,
        std::time::Duration::from_millis(u64::from(timeout_ms)),
    );
    copy_out(&json, out, &mut written)
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
/// `document` is null or open. `path` is null or addresses `path_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_audition(
    document: *const Document,
    path: *const u8,
    path_len: usize,
    input: u32,
    input_delay_us: u64,
) -> c_int {
    // SAFETY: this function's contract is `echode_document_audition_at`'s.
    unsafe { echode_document_audition_at(document, path, path_len, 0, input, input_delay_us) }
}

/// Like `echode_document_audition`, starting at `start_frame` in the source
/// file's sample rate. Positions at or beyond EOF clamp to its last sample.
/// A null or empty path stops, irrespective of the position.
///
/// # Safety
/// `document` is null or open. `path` is null or addresses `path_len`
/// readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_audition_at(
    document: *const Document,
    path: *const u8,
    path_len: usize,
    start_frame: u64,
    input: u32,
    input_delay_us: u64,
) -> c_int {
    // SAFETY: the caller passes null or an open document, and `path_len`
    // readable bytes at a non-null `path`.
    let (Some(document), Some(bytes)) =
        (unsafe { (abi::handle(document), abi::input(path, path_len)) })
    else {
        return ECHODE_ERR;
    };
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
    if bytes.is_empty() {
        document.service.audition(None, origin, 0);
        return ECHODE_OK;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ECHODE_ERR;
    };
    document
        .service
        .audition(Some(std::path::PathBuf::from(text)), origin, start_frame);
    ECHODE_OK
}

/// Keeps a silent audition output while the browser is visible and sets its
/// linear gain (0..=1). Returns immediately without the document worker.
/// This changes no project data and starts no sound. Other `visible` values,
/// non-finite gains and gains outside the range are refused without changes.
///
/// # Safety
/// `document` is null or open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_audition_configure(
    document: *const Document,
    visible: u32,
    gain: f32,
) -> c_int {
    // SAFETY: the caller passes null or an open document.
    let Some(document) = (unsafe { abi::handle(document) }) else {
        return ECHODE_ERR;
    };
    if visible > 1 || !document.service.configure_audition(visible == 1, gain) {
        return ECHODE_ERR;
    }
    ECHODE_OK
}

/// Reads the live playhead without submitting a command or taking a lock.
///
/// # Safety
/// `document` is null or open. `out` is null or a writable `TransportPosition`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_position(
    document: *const Document,
    out: *mut TransportPosition,
) -> c_int {
    // SAFETY: the caller passes null or an open document, and a writable `out`.
    let (Some(document), Some(mut out)) = (unsafe { (abi::handle(document), abi::place(out)) })
    else {
        return ECHODE_ERR;
    };
    out.set(document.position());
    ECHODE_OK
}

/// Borrowed UTF-8 JSON session state for this immutable snapshot.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// null or writable. The bytes are valid only until this snapshot is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_audio(
    snapshot: *const Snapshot,
    len: *mut usize,
) -> *const u8 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe { lend(snapshot, len, |snapshot| Some(snapshot.audio())) }
}

/// UTF-8 JSON `{revisions, changed}` from a history or perform completion.
/// Perform completions report whether they recorded an edit, with no revisions.
/// Other snapshots carry an empty list and `changed: false`. The pointer is valid
/// until the snapshot is freed.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_history(
    snapshot: *const Snapshot,
    len: *mut usize,
) -> *const u8 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe { lend(snapshot, len, |snapshot| Some(snapshot.history())) }
}

use std::ffi::{CStr, c_char, c_void};
use std::os::raw::c_int;

use echode_document::Operation;
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

/// Appends later panics to the NDJSON file at `path` (ADR 0238). An audio
/// callback's panic stops at the callback and reaches the log, with its
/// location, from the thread that reports the stopped output. Only the first
/// call installs the hook; null keeps the callbacks' capture without a file.
///
/// # Safety
/// `path` is a nullable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_keep_panic_log(path: *const c_char) {
    // SAFETY: the caller passes null or a NUL-terminated string.
    let path = unsafe { abi::text(path) };
    echode_audio_io::keep_panic_log(path.map(std::path::PathBuf::from));
}

/// Creates an empty package at `path` and opens it. Null if that fails.
///
/// # Safety
/// `path` is a nullable NUL-terminated string. The returned pointer is freed
/// with [`echode_document_close`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_create(path: *const c_char) -> *mut Document {
    // SAFETY: the caller passes null or a NUL-terminated string.
    let Some(path) = (unsafe { abi::text(path) }) else {
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
    // SAFETY: the caller passes null or a NUL-terminated string.
    let Some(path) = (unsafe { abi::text(path) }) else {
        return std::ptr::null_mut();
    };
    Document::open(std::path::Path::new(&path))
        .map(|document| Box::into_raw(Box::new(document)))
        .unwrap_or(std::ptr::null_mut())
}

/// Finishes queued commands, then rejects later submissions.
///
/// # Safety
/// `document` is null, or came from [`echode_document_open`] or
/// [`echode_document_create`] and has not been closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_shutdown(document: *mut Document) {
    // SAFETY: the caller passes null or an open document.
    if let Some(document) = unsafe { abi::handle(document) } {
        document.service.shutdown();
    }
}

/// Drops the document. A snapshot taken earlier stays readable.
///
/// # Safety
/// `document` is null, or came from [`echode_document_open`] or
/// [`echode_document_create`] and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_close(document: *mut Document) {
    if !document.is_null() {
        // SAFETY: the document came from `Box::into_raw` in `echode_document_open`
        // or `echode_document_create`, and the caller never uses it again.
        drop(unsafe { Box::from_raw(document) });
    }
}

/// Queues `json` and returns. `complete` runs once with `context`.
///
/// `complete` is required and runs once, which hands `context` back.
///
/// A null document, a null `json` with a nonzero `json_len`, or more than
/// `isize::MAX` bytes complete at once with `ECHODE_ERR`, which is also returned.
///
/// # Safety
/// `document` is null or open. `json` is null or addresses `json_len`
/// readable bytes, not necessarily NUL-terminated. `context` is not read by Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_document_submit(
    document: *mut Document,
    json: *const u8,
    json_len: usize,
    context: *mut c_void,
    complete: Complete,
) -> c_int {
    // SAFETY: this function's contract is `queue`'s.
    unsafe {
        queue(
            document,
            json,
            json_len,
            context,
            complete,
            Operation::Perform,
            c"command is null or too long",
        )
    }
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
    // SAFETY: the caller passes null or an open document.
    let Some(document) = (unsafe { abi::handle(document) }) else {
        return refuse(context, complete, c"document is null");
    };
    let operation = match operation {
        ECHODE_DOCUMENT_SNAPSHOT => Operation::Snapshot,
        ECHODE_DOCUMENT_UNDO => Operation::Undo,
        ECHODE_DOCUMENT_REDO => Operation::Redo,
        _ => Operation::Invalid,
    };
    document.enqueue(operation, context, complete);
    ECHODE_OK
}

/// Another owned handle for the same project.
///
/// # Safety
/// `snapshot` is null, or came from a completion or from this function and
/// has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_retain(snapshot: *mut Snapshot) -> *mut Snapshot {
    // SAFETY: the caller passes null or an owned handle.
    unsafe { abi::handle(snapshot) }
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
        // SAFETY: every handle this library returns comes from `Box::into_raw`,
        // and the caller frees each one once.
        drop(unsafe { Box::from_raw(snapshot) });
    }
}

/// How many tracks the snapshot contains. Null is zero.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_count(snapshot: *const Snapshot) -> usize {
    // SAFETY: the caller passes null or an owned handle.
    unsafe { abi::handle(snapshot) }
        .map(Snapshot::track_count)
        .unwrap_or(0)
}

/// Writes the UUID bytes of track `index`. Returns 0, or `-1` when it is
/// missing or `out` is null; `out` is unchanged then.
///
/// # Safety
/// `out` is null or points at 16 writable bytes. `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_id(
    snapshot: *const Snapshot,
    index: usize,
    out: *mut u8,
) -> c_int {
    // SAFETY: the caller passes null or an owned handle.
    let id = unsafe { abi::handle(snapshot) }.and_then(|snapshot| snapshot.track_id(index));
    // SAFETY: the caller passes 16 writable bytes at a non-null `out`.
    write_uuid(id, unsafe { abi::place(out.cast::<[u8; 16]>()) })
}

/// Borrows the UTF-8 name of track `index`, without a trailing NUL. Missing
/// tracks return null and length zero. No model storage is copied or mutated.
///
/// # Safety
/// `length` is null or writable. `snapshot` is null or owned; keep it alive for the
/// entire read of the returned `length` bytes, and never write through them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_track_name(
    snapshot: *const Snapshot,
    index: usize,
    length: *mut usize,
) -> *const u8 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe {
        lend(snapshot, length, |snapshot| {
            snapshot.track_name(index).map(str::as_bytes)
        })
    }
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
    // SAFETY: the caller passes null or an owned handle.
    unsafe { abi::handle(snapshot) }
        .map(|snapshot| snapshot.device_count(track))
        .unwrap_or(0)
}

/// Writes the UUID bytes of a device. Returns 0, or `-1` when it is missing
/// or `out` is null; `out` is unchanged then.
///
/// # Safety
/// `out` is null or points at 16 writable bytes. `snapshot` is null or still owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_id(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    out: *mut u8,
) -> c_int {
    // SAFETY: the caller passes null or an owned handle.
    let id =
        unsafe { abi::handle(snapshot) }.and_then(|snapshot| snapshot.device_id(track, device));
    // SAFETY: the caller passes 16 writable bytes at a non-null `out`.
    write_uuid(id, unsafe { abi::place(out.cast::<[u8; 16]>()) })
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
    // SAFETY: the caller passes null or an owned handle.
    unsafe { abi::handle(snapshot) }
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
    // SAFETY: the caller passes null or an owned handle.
    unsafe { abi::handle(snapshot) }
        .and_then(|snapshot| snapshot.device_offline(track, device))
        .map_or(-1, c_int::from)
}

/// Borrows the CLAP id of a device. The mixer has none and returns null.
///
/// # Safety
/// `length` is null or writable. `snapshot` is null or owned for the whole
/// read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_plugin(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    length: *mut usize,
) -> *const u8 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe {
        lend(snapshot, length, |snapshot| {
            snapshot.device_plugin(track, device).map(str::as_bytes)
        })
    }
}

/// Borrows the ascending CLAP parameter ids pinned on a device. An invalid
/// snapshot or index returns null with a zero count; no pins return a count of 0.
///
/// # Safety
/// `count` is null or writable. `snapshot` is null or owned for the whole
/// read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_snapshot_device_pins(
    snapshot: *const Snapshot,
    track: usize,
    device: usize,
    count: *mut usize,
) -> *const u32 {
    // SAFETY: this function's contract is `lend`'s.
    unsafe {
        lend(snapshot, count, |snapshot| {
            snapshot.device_pins(track, device)
        })
    }
}

/// Writes the installed CLAP catalog as UTF-8 JSON.
///
/// Returns 0 when `out` holds the document. Returns 1 when `capacity` is too
/// small and `written` is the required size. Returns `-1` when the helper
/// cannot be started, or, before scanning, for a null `written` or a refused
/// `out` (see `abi`). The helper process loads each bundle.
///
/// # Safety
/// `written` is null or writable. `out` is null or has room for `capacity`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_plugin_catalog(
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> c_int {
    // SAFETY: the caller passes a writable `written`, and room for
    // `capacity` bytes at a non-null `out`.
    let (Some(mut written), Some(out)) =
        (unsafe { (abi::place(written), abi::output(out, capacity)) })
    else {
        return -1;
    };
    written.set(0);
    let Ok(json) = catalog::plugins_json() else {
        return -1;
    };
    copy_out(&json, out, &mut written)
}

/// Reads the whole audio file at `path` (UTF-8) and writes its overview as
/// UTF-8 JSON: `{"waveform": ...}` with the audition's peak fields,
/// `{"error": "..."}`, or `{"cancelled": true}` when a call on the same
/// `channel` with a larger `request` started meanwhile, or the channel ended.
/// Each browser uses its own channel. Blocks while it reads, so call it off
/// the main thread. It opens no audio output and touches no document.
///
/// Returns 0 when `out` holds the document. Returns 1 when `capacity` is too
/// small and `written` is the required size; the next call reads again.
/// Returns `-1` for an empty or invalid path, a null `written`, or a refused
/// `out` (see `abi`).
///
/// # Safety
/// `written` is null or writable. `path` is null or addresses `path_len`
/// readable bytes. `out` is null or has room for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_sample_overview(
    path: *const u8,
    path_len: usize,
    channel: u64,
    request: u64,
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> c_int {
    // SAFETY: the caller passes a writable `written`, `path_len` readable
    // bytes at a non-null `path`, and room for `capacity` bytes at a non-null `out`.
    let (Some(mut written), Some(bytes), Some(out)) = (unsafe {
        (
            abi::place(written),
            abi::input(path, path_len),
            abi::output(out, capacity),
        )
    }) else {
        return -1;
    };
    written.set(0);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return -1;
    };
    if text.is_empty() {
        return -1;
    }
    let json = overview::overview_json(std::path::Path::new(text), channel, request);
    copy_out(&json, out, &mut written)
}

/// Stops the reads of an overview channel whose browser went away.
///
/// # Safety
/// There are no pointer arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn echode_sample_overview_end(channel: u64) {
    overview::end(channel);
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
/// every element as dropped and does not allocate. Returns -1, with zero
/// counts, for a null snapshot or a refused `out` (see `abi`).
///
/// # Safety
/// `out` is null or has room for `capacity` elements. `snapshot` is null or
/// still owned. `written` and `dropped` are null or writable and do not alias.
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
    // SAFETY: the caller passes writable, unaliased counts.
    let (Some(mut written), Some(mut dropped)) =
        (unsafe { (abi::place(written), abi::place(dropped)) })
    else {
        return -1;
    };
    written.set(0);
    dropped.set(0);
    // SAFETY: the caller passes null or an owned handle, and room for
    // `capacity` elements at a non-null `out`.
    let (Some(snapshot), Some(slot)) =
        (unsafe { (abi::handle(snapshot), abi::output(out, capacity)) })
    else {
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
    let (filled, overflow) = snapshot.write_visible(view, slot);
    written.set(filled);
    dropped.set(overflow);
    0
}

/// Writes min and max for each visible column of one audio placement.
///
/// `placement` is 16 UUID bytes. The tick window is the clip rectangle's
/// time. A note placement, a missing file or a refused argument (see `abi`)
/// writes nothing and returns 0.
///
/// # Safety
/// `snapshot` is null or still owned. `placement` is null or addresses 16
/// readable bytes. `mins` and `maxs` are each null or hold
/// `capacity` writable floats for the whole call, and they do not overlap.
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
    // SAFETY: the caller passes null or an owned handle, null or 16 readable
    // bytes at `placement`, and two separate rooms for `capacity` floats.
    let (Some(snapshot), Some(placement), Some(mins), Some(maxs)) = (unsafe {
        (
            abi::handle(snapshot),
            abi::uuid(placement),
            abi::output(mins, capacity),
            abi::output(maxs, capacity),
        )
    }) else {
        return 0;
    };
    // The caller's room bounds the columns, but not what this process can
    // allocate: a capacity it cannot hold writes nothing instead of aborting.
    let mut columns = Vec::new();
    if capacity == 0 || columns.try_reserve_exact(capacity).is_err() {
        return 0;
    }
    columns.resize(capacity, echode_history::PeakColumn { min: 0.0, max: 0.0 });
    let placement = echode_model::Id::from_raw(placement);
    let written = snapshot.waveform(placement, start_tick, end_tick, &mut columns);
    for ((column, min), max) in columns.iter().take(written).zip(mins).zip(maxs) {
        min.write(column.min);
        max.write(column.max);
    }
    written
}

/// Completes a refused submission at once, as every queued entry point does.
fn refuse(context: *mut c_void, complete: Complete, message: &CStr) -> c_int {
    complete(context, ECHODE_ERR, message.as_ptr(), std::ptr::null_mut());
    ECHODE_ERR
}

/// Queues a JSON command for the document thread, or completes it at once with
/// `refused` when `json` and `json_len` are refused (see `abi`).
///
/// # Safety
/// `document` is null or open. `json` is null or addresses `json_len`
/// readable bytes.
unsafe fn queue(
    document: *mut Document,
    json: *const u8,
    json_len: usize,
    context: *mut c_void,
    complete: Complete,
    operation: fn(Vec<u8>) -> Operation,
    refused: &CStr,
) -> c_int {
    // SAFETY: the caller passes null or an open document.
    let Some(document) = (unsafe { abi::handle(document) }) else {
        return refuse(context, complete, c"document is null");
    };
    // SAFETY: the caller passes `json_len` readable bytes at a non-null `json`.
    let Some(bytes) = (unsafe { abi::input(json, json_len) }) else {
        return refuse(context, complete, refused);
    };
    document.enqueue(operation(bytes.to_vec()), context, complete);
    ECHODE_OK
}

/// Lends values a snapshot owns: their address, with their count in `len`.
/// A null `len` returns null. A null snapshot or no values returns null with
/// a zero count.
///
/// # Safety
/// `snapshot` is null or an owned handle that has not been freed. `len` is
/// null or writable. The values stay valid until the snapshot is freed.
unsafe fn lend<'a, T: 'a>(
    snapshot: *const Snapshot,
    len: *mut usize,
    read: impl FnOnce(&'a Snapshot) -> Option<&'a [T]>,
) -> *const T {
    // SAFETY: the caller passes null or a writable `len`.
    let Some(mut len) = (unsafe { abi::place(len) }) else {
        return std::ptr::null();
    };
    len.set(0);
    // SAFETY: the caller passes null or an owned handle.
    let Some(values) = (unsafe { abi::handle(snapshot) }).and_then(read) else {
        return std::ptr::null();
    };
    len.set(values.len());
    values.as_ptr()
}

/// Copies a reply into the caller's room and reports its size in `written`.
/// Returns 0 when copied, or 1, copying nothing, when the room is too small.
fn copy_out(
    bytes: &[u8],
    out: &mut [std::mem::MaybeUninit<u8>],
    written: &mut abi::Place<'_, usize>,
) -> c_int {
    written.set(bytes.len());
    if abi::fill(out, bytes) { 0 } else { 1 }
}

/// Writes a UUID through an out-parameter. -1 when either is missing.
fn write_uuid(id: Option<[u8; 16]>, out: Option<abi::Place<'_, [u8; 16]>>) -> c_int {
    match (id, out) {
        (Some(id), Some(mut out)) => {
            out.set(id);
            0
        }
        _ => -1,
    }
}

#[cfg(test)]
mod tests;

// Pure snapshot calls that also run under Miri.
#[cfg(test)]
mod abi_tests;

#[cfg(test)]
mod snapshot_fields_tests;

#[cfg(test)]
mod completion_tests;

#[cfg(test)]
mod opening_tests;

#[cfg(test)]
mod recovery_tests;
// Pure snapshot tests also run under Miri. No package IO, UUID randomness, or OS
// threads are needed to check borrowed names, retained ownership and capacities.
#[cfg(test)]
mod snapshot_ownership;
