use super::*;
use std::ffi::{CStr, CString};
use std::sync::atomic::AtomicUsize;

fn push(log: &PluginLog, severity: clap_log_severity, text: &CStr) {
    let _guard = crate::realtime::ForbidAlloc::enter();
    // SAFETY: the borrowed C string is valid until the call returns.
    unsafe { log.push(severity, text.as_ptr()) };
}

#[test]
fn records_own_the_prefix_and_preserve_raw_severity_and_bytes() {
    let log = PluginLog::new();
    let mut text = b"owned\0".to_vec();
    // SAFETY: `text` is NUL-terminated and live until after the call.
    unsafe { log.push(99, text.as_ptr().cast()) };
    text.fill(b'x');
    push(&log, CLAP_LOG_INFO, c"");
    push(&log, CLAP_LOG_WARNING, c"\xff");
    // SAFETY: a null pointer is explicitly ignored, with no dereference.
    unsafe { log.push(CLAP_LOG_DEBUG, std::ptr::null()) };
    let mut records = Vec::new();
    assert_eq!(log.drain(|record| records.push(record)), 0);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].severity, 99);
    assert_eq!(&records[0].text[..records[0].len], b"owned");
    assert_eq!(records[1].len, 0);
    assert_eq!(&records[2].text[..records[2].len], &[255]);
    assert!(records.iter().all(|record| !record.truncated));
}

#[test]
fn diagnostic_prefix_copy_stops_at_nul_or_the_byte_budget() {
    let log = PluginLog::new();
    for len in [TEXT_BYTES - 1, TEXT_BYTES, TEXT_BYTES + 1] {
        let text = CString::new(vec![b'x'; len]).unwrap();
        push(&log, CLAP_LOG_INFO, &text);
        let mut records = Vec::new();
        assert_eq!(log.drain(|record| records.push(record)), 0);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].len, len.min(TEXT_BYTES));
        assert_eq!(records[0].truncated, len >= TEXT_BYTES);
        assert!(records[0].text[..records[0].len].iter().all(|&b| b == b'x'));
    }
}

#[test]
fn full_and_contended_slots_drop_without_waiting_and_recover_after_drain() {
    let log = PluginLog::new();
    for _ in 0..SLOTS + 3 {
        push(&log, CLAP_LOG_INFO, c"overflow");
    }
    let mut count = 0;
    assert_eq!(log.drain(|_| count += 1), 3);
    assert_eq!(count, SLOTS);
    assert_eq!(log.drain(|_| panic!("already drained")), 0);
    let position = log.next.load(Ordering::Relaxed) % SLOTS;
    // Simulate a suspended writer. Another producer must not wait or retry.
    log.slots[position].state.store(BUSY, Ordering::Relaxed);
    push(&log, CLAP_LOG_WARNING, c"contended");
    assert_eq!(log.drain(|_| panic!("busy slot is not readable")), 1);
    log.slots[position].state.store(EMPTY, Ordering::Release);
    push(&log, CLAP_LOG_ERROR, c"recovered");
    assert_eq!(
        log.drain(|record| assert_eq!(record.severity, CLAP_LOG_ERROR)),
        0
    );
}

#[test]
fn concurrent_producers_and_a_consumer_preserve_records_and_account_for_every_drop() {
    // Counts define this concurrency fixture only, not product limits.
    const PRODUCERS: usize = 4;
    const MESSAGES: usize = 500;
    let log = PluginLog::new();
    let remaining = AtomicUsize::new(PRODUCERS);
    let mut received = 0;
    let mut dropped = 0;
    std::thread::scope(|scope| {
        for id in 0..PRODUCERS {
            let log = &log;
            let remaining = &remaining;
            scope.spawn(move || {
                let message = CString::new(format!("producer-{id}")).unwrap();
                for _ in 0..MESSAGES {
                    push(log, id as i32, &message);
                }
                remaining.fetch_sub(1, Ordering::Release);
            });
        }
        let mut consume = |record: Record| {
            assert!((0..PRODUCERS as i32).contains(&record.severity));
            let expected = format!("producer-{}", record.severity);
            assert_eq!(&record.text[..record.len], expected.as_bytes());
            assert!(!record.truncated);
            received += 1;
        };
        while remaining.load(Ordering::Acquire) != 0 {
            dropped += log.drain(&mut consume);
            std::thread::yield_now();
        }
        dropped += log.drain(consume);
    });
    assert!(received > 0);
    assert_eq!(received + dropped, (PRODUCERS * MESSAGES) as u64);
}
