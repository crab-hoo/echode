//! Generated declarations live in target/bridge/c; no hand-written C header.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::{BridgeDocument, BridgeError, BridgeNote, BridgeSnapshot, LiveState};

pub struct CBridgeDocument {
    inner: Arc<BridgeDocument>,
}

pub struct CBridgeSnapshot {
    inner: Arc<BridgeSnapshot>,
}

impl CBridgeSnapshot {
    fn owned(inner: Arc<BridgeSnapshot>) -> *mut Self {
        inner.live.snapshot_handles.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(Self { inner }))
    }
}

impl Drop for CBridgeSnapshot {
    fn drop(&mut self) {
        self.inner
            .live
            .snapshot_handles
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum BridgeStatus {
    Ok = 0,
    EmptyName = 1,
    InvalidUtf8 = 2,
    Stopped = 3,
}

pub type BridgeCompletion = unsafe extern "C" fn(*mut c_void, BridgeStatus, *mut CBridgeSnapshot);

// The token is never dereferenced in Rust. Ownership returns to Swift exactly
// once via the completion function, even when enqueueing fails.
struct Completion {
    token: Option<(*mut c_void, BridgeCompletion)>,
    live: Arc<LiveState>,
}

// SAFETY: the caller promises the context may be passed to the control thread.
unsafe impl Send for Completion {}

impl Completion {
    fn finish(mut self, status: BridgeStatus, snapshot: *mut CBridgeSnapshot) {
        let (context, function) = self.token.take().unwrap();
        // SAFETY: the foreign caller keeps the function and context alive until here.
        unsafe { function(context, status, snapshot) };
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if let Some((context, function)) = self.token.take() {
            // SAFETY: an abandoned accepted command still owns its foreign context.
            // This fallback is outside queue and publication locks.
            unsafe { function(context, BridgeStatus::Stopped, ptr::null_mut()) };
        }
        self.live.completions.fetch_sub(1, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn eb_document_new(note_count: u32) -> *mut CBridgeDocument {
    catch_unwind(AssertUnwindSafe(|| {
        Box::into_raw(Box::new(CBridgeDocument {
            inner: BridgeDocument::new(note_count),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// `document` is an owned handle from `eb_document_new`, released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_free(document: *mut CBridgeDocument) {
    if !document.is_null() {
        // SAFETY: ownership of this Box returns exactly once.
        drop(unsafe { Box::from_raw(document) });
    }
}

/// # Safety
/// `document` remains live throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_snapshot(
    document: *const CBridgeDocument,
) -> *mut CBridgeSnapshot {
    // SAFETY: the caller owns a live document handle.
    let document = unsafe { &*document };
    CBridgeSnapshot::owned(document.inner.snapshot())
}

/// # Safety
/// `document` remains live throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_live_snapshots(document: *const CBridgeDocument) -> u64 {
    // SAFETY: the caller owns a live document handle.
    unsafe { &*document }.inner.live_snapshots()
}

/// Enqueues work and returns immediately. Completion runs on the document thread
/// (or inline on invalid UTF-8 / a stopped queue). Its snapshot is an owned handle.
/// # Safety
/// `document` and `name[0..name_len]` are live for this call. `context` and the
/// completion remain live until called, may cross threads, and must not unwind.
/// `name` may be null when `name_len` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_add_track(
    document: *const CBridgeDocument,
    name: *const u8,
    name_len: usize,
    context: *mut c_void,
    completion: BridgeCompletion,
) {
    // SAFETY: the caller keeps the document alive for this call.
    let live = Arc::clone(&unsafe { &*document }.inner.live);
    live.completions.fetch_add(1, Ordering::Relaxed);
    let completion = Completion {
        token: Some((context, completion)),
        live,
    };
    let bytes = if name_len == 0 {
        &[]
    } else {
        // SAFETY: the caller lends a readable buffer for this call only.
        unsafe { std::slice::from_raw_parts(name, name_len) }
    };
    let Ok(name) = std::str::from_utf8(bytes) else {
        completion.finish(BridgeStatus::InvalidUtf8, ptr::null_mut());
        return;
    };
    // SAFETY: the caller owns a live document handle. No reference escapes.
    unsafe { &*document }.inner.enqueue(
        name.to_owned(),
        Box::new(move |result| match result {
            Ok(inner) => completion.finish(BridgeStatus::Ok, CBridgeSnapshot::owned(inner)),
            Err(error) => completion.finish(
                match error {
                    BridgeError::EmptyName => BridgeStatus::EmptyName,
                    BridgeError::Stopped => BridgeStatus::Stopped,
                },
                ptr::null_mut(),
            ),
        }),
    );
}

/// # Safety
/// `snapshot` is live for this call. The returned handle must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_retain(
    snapshot: *const CBridgeSnapshot,
) -> *mut CBridgeSnapshot {
    // SAFETY: the input owns a live Arc; retaining it does not clone its payload.
    let inner = Arc::clone(&unsafe { &*snapshot }.inner);
    CBridgeSnapshot::owned(inner)
}

/// # Safety
/// `snapshot` is an owned handle, released exactly once; null is accepted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_free(snapshot: *mut CBridgeSnapshot) {
    if !snapshot.is_null() {
        // SAFETY: ownership of this Box returns exactly once.
        drop(unsafe { Box::from_raw(snapshot) });
    }
}

/// # Safety
/// Both snapshots remain live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_shares_project(
    left: *const CBridgeSnapshot,
    right: *const CBridgeSnapshot,
) -> bool {
    // SAFETY: both inputs own live Arcs while ptr_eq examines them.
    let (left, right) = unsafe { (&*left, &*right) };
    left.inner.shares_project(Arc::clone(&right.inner))
}

/// # Safety
/// `snapshot` remains live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_revision(snapshot: *const CBridgeSnapshot) -> u64 {
    // SAFETY: the caller owns a live snapshot handle.
    unsafe { &*snapshot }.inner.revision()
}

/// # Safety
/// `snapshot` remains live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_track_count(snapshot: *const CBridgeSnapshot) -> u64 {
    // SAFETY: the caller owns a live snapshot handle.
    unsafe { &*snapshot }.inner.track_count()
}

/// # Safety
/// `snapshot` remains live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_note_count(snapshot: *const CBridgeSnapshot) -> usize {
    // SAFETY: the caller owns a live snapshot handle.
    unsafe { &*snapshot }.inner.notes.len()
}

/// Borrows immutable storage; never free the returned pointer. It remains valid
/// only while the caller owns a snapshot handle. Count comes from note_count.
/// # Safety
/// Keep `snapshot` retained throughout every read of the returned buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_snapshot_notes(snapshot: *const CBridgeSnapshot) -> *const BridgeNote {
    // SAFETY: the caller retains the Arc owning this read-only allocation.
    unsafe { &*snapshot }.inner.notes.as_ptr()
}

#[cfg(feature = "fault-injection")]
mod validation;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::mpsc;

    unsafe extern "C" fn completed(
        context: *mut c_void,
        status: BridgeStatus,
        snapshot: *mut CBridgeSnapshot,
    ) {
        // SAFETY: the test transfers this sender exactly once per call.
        let reply = unsafe { Box::from_raw(context.cast::<mpsc::Sender<(BridgeStatus, u64)>>()) };
        let revision = if snapshot.is_null() {
            0
        } else {
            // SAFETY: the completion receives an owned live handle.
            let revision = unsafe { eb_snapshot_revision(snapshot) };
            unsafe { eb_snapshot_free(snapshot) };
            revision
        };
        reply.send((status, revision)).unwrap();
    }

    #[test]
    fn c_note_layout_matches_the_generated_swift_view() {
        assert_eq!(std::mem::size_of::<BridgeNote>(), 24);
        assert_eq!(std::mem::align_of::<BridgeNote>(), 8);
        assert_eq!(std::mem::offset_of!(BridgeNote, pitch), 16);
        assert_eq!(std::mem::size_of::<BridgeStatus>(), 4);
    }

    #[test]
    fn c_completion_consumes_context_on_success_and_both_input_errors() {
        let document = eb_document_new(0);
        for (name, expected) in [
            (&[0xff][..], (BridgeStatus::InvalidUtf8, 0)),
            (&[][..], (BridgeStatus::EmptyName, 0)),
            (&b"piano"[..], (BridgeStatus::Ok, 1)),
        ] {
            let (reply, incoming) = mpsc::channel::<(BridgeStatus, u64)>();
            let context = Box::into_raw(Box::new(reply)).cast();
            // SAFETY: the name is valid for the call and the callback owns context.
            unsafe {
                eb_document_add_track(document, name.as_ptr(), name.len(), context, completed);
            }
            assert_eq!(incoming.recv().unwrap(), expected);
            assert!(incoming.recv().is_err(), "completion drops its only sender");
        }
        // SAFETY: the test returns ownership of the document exactly once.
        unsafe { eb_document_free(document) };
    }

    #[test]
    fn c_retained_view_survives_original_handle_and_document() {
        let document = eb_document_new(2);
        // SAFETY: every call uses a live owned handle and frees it exactly once.
        unsafe {
            let original = eb_document_snapshot(document);
            let retained = eb_snapshot_retain(original);
            assert!(eb_snapshot_shares_project(original, retained));
            eb_snapshot_free(original);
            eb_document_free(document);
            assert_eq!(eb_snapshot_note_count(retained), 2);
            assert_eq!((*eb_snapshot_notes(retained).add(1)).start, 960);
            eb_snapshot_free(retained);
        }
    }

    #[test]
    fn c_abandoned_completion_returns_its_context_once() {
        let live = Arc::new(LiveState::default());
        live.completions.fetch_add(1, Ordering::Relaxed);
        let (reply, incoming) = mpsc::channel::<(BridgeStatus, u64)>();
        drop(Completion {
            token: Some((Box::into_raw(Box::new(reply)).cast(), completed)),
            live: Arc::clone(&live),
        });
        assert_eq!(incoming.recv().unwrap(), (BridgeStatus::Stopped, 0));
        assert!(incoming.recv().is_err());
        assert_eq!(live.completions.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn c_null_empty_input_is_an_error_with_a_completion() {
        let document = eb_document_new(0);
        let (reply, incoming) = mpsc::channel::<(BridgeStatus, u64)>();
        // SAFETY: a null pointer with zero length is an explicitly allowed input.
        unsafe {
            eb_document_add_track(
                document,
                ptr::null(),
                0,
                Box::into_raw(Box::new(reply)).cast(),
                completed,
            );
        }
        assert_eq!(incoming.recv().unwrap(), (BridgeStatus::EmptyName, 0));
        assert!(incoming.recv().is_err());
        // SAFETY: this returns the sole owned document handle.
        unsafe { eb_document_free(document) };
    }

    #[cfg(feature = "fault-injection")]
    #[test]
    fn c_worker_panic_completes_current_queued_and_future_calls() {
        let document = eb_document_new(0);
        // SAFETY: the fixture owns the document for every call and closes it once.
        unsafe {
            let live = Arc::clone(&(&*document).inner.live);
            validation::eb_document_pause(document);
            let submit = || {
                let (reply, incoming) = mpsc::channel::<(BridgeStatus, u64)>();
                eb_document_add_track(
                    document,
                    b"name".as_ptr(),
                    4,
                    Box::into_raw(Box::new(reply)).cast(),
                    completed,
                );
                incoming
            };
            let current = submit();
            validation::eb_document_wait_until_paused(document);
            let queued = submit();
            validation::eb_document_fail_next(document);
            validation::eb_document_resume(document);
            for reply in [current, queued] {
                assert_eq!(reply.recv().unwrap(), (BridgeStatus::Stopped, 0));
                assert!(reply.recv().is_err());
            }
            let refused = submit();
            assert_eq!(refused.recv().unwrap(), (BridgeStatus::Stopped, 0));
            assert!(refused.recv().is_err());
            assert_eq!((&*document).inner.snapshot().revision(), 0);
            eb_document_free(document);
            live.wait_for_worker();
            assert_eq!(live.completions.load(Ordering::Relaxed), 0);
            assert_eq!(live.snapshot_handles.load(Ordering::Relaxed), 0);
            assert_eq!(live.snapshots.load(Ordering::Relaxed), 0);
        }
    }

    proptest! {
        // Case/sequence sizes are this validation's budget, never API limits.
        #![proptest_config(ProptestConfig {
            cases: if cfg!(miri) { 4 } else { 64 },
            failure_persistence: if cfg!(miri) { None } else { ProptestConfig::default().failure_persistence },
            ..ProptestConfig::default()
        })]
        #[test]
        fn c_valid_ownership_sequences_release_every_resource(
            steps in prop::collection::vec(any::<u8>(), 1..32),
            bytes in prop::collection::vec(any::<u8>(), 0..24),
        ) {
            let document = eb_document_new(4);
            let mut handles = Vec::new();
            let mut revision = 0;
            // SAFETY: the document is live; counters do not own the document itself.
            let live = Arc::clone(&unsafe { &*document }.inner.live);
            for step in steps {
                // SAFETY: all handles are generated by the API, retained before use,
                // and removed from this collection when their ownership is returned.
                unsafe {
                    match step % 5 {
                        0 => handles.push((eb_document_snapshot(document), revision)),
                        1 if !handles.is_empty() => {
                            let (source, expected) = handles[usize::from(step) % handles.len()];
                            handles.push((eb_snapshot_retain(source), expected));
                        }
                        2 => {
                            if let Some((handle, _)) = handles.pop() { eb_snapshot_free(handle); }
                        }
                        3 => {
                            let (reply, incoming) = mpsc::channel::<(BridgeStatus, u64)>();
                            eb_document_add_track(document, bytes.as_ptr(), bytes.len(),
                                Box::into_raw(Box::new(reply)).cast(), completed);
                            let status = match std::str::from_utf8(&bytes) {
                                Err(_) => BridgeStatus::InvalidUtf8,
                                Ok("") => BridgeStatus::EmptyName,
                                Ok(_) => { revision += 1; BridgeStatus::Ok }
                            };
                            let expected = if status == BridgeStatus::Ok { revision } else { 0 };
                            prop_assert_eq!(incoming.recv().unwrap(), (status, expected));
                            prop_assert!(incoming.recv().is_err());
                        }
                        _ => {
                            for &(handle, expected) in &handles {
                                prop_assert_eq!(eb_snapshot_revision(handle), expected);
                                prop_assert_eq!(eb_snapshot_note_count(handle), 4);
                                prop_assert_eq!((*eb_snapshot_notes(handle).add(3)).start, 2880);
                            }
                        }
                    }
                }
                prop_assert_eq!(live.snapshot_handles.load(Ordering::Relaxed), handles.len());
            }
            // SAFETY: release the document while the owned views remain live.
            unsafe { eb_document_free(document) };
            live.wait_for_worker();
            prop_assert_eq!(live.documents.load(Ordering::Relaxed), 0);
            prop_assert_eq!(live.workers.load(Ordering::Relaxed), 0);
            prop_assert_eq!(live.completions.load(Ordering::Relaxed), 0);
            for (handle, expected) in handles {
                // SAFETY: each surviving view owns its immutable storage independently.
                unsafe {
                    prop_assert_eq!(eb_snapshot_revision(handle), expected);
                    prop_assert_eq!((*eb_snapshot_notes(handle)).length, 480);
                    eb_snapshot_free(handle);
                }
            }
            prop_assert_eq!(live.snapshot_handles.load(Ordering::Relaxed), 0);
            prop_assert_eq!(live.snapshots.load(Ordering::Relaxed), 0);
        }
    }
}
