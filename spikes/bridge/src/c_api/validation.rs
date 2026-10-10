//! Deterministic controls and resource accounting for the UI-2 validation only.

use super::*;

pub struct CBridgeProbe {
    live: Arc<LiveState>,
}

#[repr(C)]
pub struct BridgeCounts {
    pub documents: u64,
    pub workers: u64,
    pub snapshots: u64,
    pub snapshot_handles: u64,
    pub completions: u64,
}

/// # Safety
/// `document` is live. The returned probe owns its counters and must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_probe_new(document: *const CBridgeDocument) -> *mut CBridgeProbe {
    // SAFETY: the caller owns the document throughout this call.
    Box::into_raw(Box::new(CBridgeProbe {
        live: Arc::clone(&unsafe { &*document }.inner.live),
    }))
}

/// # Safety
/// `probe` is an owned handle from eb_probe_new, released once; null is accepted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_probe_free(probe: *mut CBridgeProbe) {
    if !probe.is_null() {
        // SAFETY: the caller returns ownership exactly once.
        drop(unsafe { Box::from_raw(probe) });
    }
}

/// # Safety
/// `probe` is live. The counts are diagnostic, not synchronization primitives.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_probe_counts(probe: *const CBridgeProbe) -> BridgeCounts {
    // SAFETY: the caller keeps the probe alive.
    let live = &unsafe { &*probe }.live;
    BridgeCounts {
        documents: live.documents.load(Ordering::Relaxed) as u64,
        workers: live.workers.load(Ordering::Relaxed) as u64,
        snapshots: live.snapshots.load(Ordering::Relaxed) as u64,
        snapshot_handles: live.snapshot_handles.load(Ordering::Relaxed) as u64,
        completions: live.completions.load(Ordering::Relaxed) as u64,
    }
}

/// # Safety
/// `probe` is live. Close the queue or release the document before waiting.
/// Call only from the validation driver, never from a completion or audio thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_probe_wait_for_worker(probe: *const CBridgeProbe) {
    // SAFETY: the caller retains the probe during the wait.
    unsafe { &*probe }.live.wait_for_worker();
}

/// # Safety
/// `document` remains live throughout this call. Validation only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_pause(document: *const CBridgeDocument) {
    // SAFETY: the driver retains its document.
    let mut state = unsafe { &*document }.inner.gate.state.lock().unwrap();
    state.paused = true;
    state.entered = false;
}

/// # Safety
/// `document` is live and has a submitted command. Validation driver only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_wait_until_paused(document: *const CBridgeDocument) {
    // SAFETY: the driver retains its document.
    let gate = &unsafe { &*document }.inner.gate;
    let mut state = gate.state.lock().unwrap();
    while !state.entered {
        state = gate.wake.wait(state).unwrap();
    }
}

/// # Safety
/// `document` remains live throughout this call. Validation only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_resume(document: *const CBridgeDocument) {
    // SAFETY: the driver retains its document.
    let gate = &unsafe { &*document }.inner.gate;
    gate.state.lock().unwrap().paused = false;
    gate.wake.notify_all();
}

/// # Safety
/// `document` is live. Inject only a pre-mutation worker panic in this spike.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_fail_next(document: *const CBridgeDocument) {
    // SAFETY: the driver retains its document.
    unsafe { &*document }.inner.gate.state.lock().unwrap().fail = true;
}

/// # Safety
/// `document` is live. Accepted commands still drain; new commands are refused.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eb_document_close_queue(document: *const CBridgeDocument) {
    // SAFETY: the driver retains its document.
    unsafe { &*document }.inner.close_queue();
}
