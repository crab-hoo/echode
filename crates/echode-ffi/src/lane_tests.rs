use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc;
use std::time::Duration;

use echode_model::Project;

use super::super::{Context, Operation};
use super::*;
use crate::ECHODE_ERR;

type Completion = (i32, Option<String>, Option<Vec<u8>>);

extern "C" fn complete(
    context: *mut c_void,
    status: i32,
    message: *const c_char,
    snapshot: *mut Snapshot,
) {
    // SAFETY: each test job owns one boxed sender and completes exactly once.
    let sender = unsafe { Box::from_raw(context.cast::<mpsc::Sender<Completion>>()) };
    let message = (!message.is_null())
        // SAFETY: a non-null message is a NUL-terminated string for this call.
        .then(|| {
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        });
    let report = (!snapshot.is_null())
        // SAFETY: a completed snapshot is handed to the callee.
        .then(|| unsafe { Box::from_raw(snapshot) }.device().to_vec());
    let _ = sender.send((status, message, report));
}

fn job() -> (Job, mpsc::Receiver<Completion>) {
    let (sender, done) = mpsc::channel();
    let job = Job {
        operation: Operation::Snapshot,
        context: Context(Box::into_raw(Box::new(sender)).cast()),
        complete,
        transport_revision: 0,
    };
    (job, done)
}

fn snapshot() -> Snapshot {
    Snapshot::from_project(Arc::new(Project::new()), None)
}

/// A round trip that waits for the test, and tells it when it started.
fn held(reply: Result<Vec<u8>, String>) -> (RoundTrip, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (started, reached) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let round_trip: RoundTrip = Box::new(move || {
        let _ = started.send(());
        let _ = released.recv();
        reply
    });
    (round_trip, reached, release)
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn a_held_round_trip_returns_to_the_worker_at_once_and_reports_later() {
    let mut lane = DeviceLane::start().unwrap();
    let (first, done) = job();
    let (round_trip, reached, release) = held(Ok(br#"{"parameters":[]}"#.to_vec()));
    lane.report(first, snapshot(), round_trip);
    reached.recv_timeout(WAIT).unwrap();
    // The worker keeps going: nothing is returned and nothing completed.
    assert!(lane.take_returned().is_none() && !lane.waiting());
    assert!(done.try_recv().is_err());
    release.send(()).unwrap();
    let (status, message, report) = done.recv_timeout(WAIT).unwrap();
    assert_eq!((status, message), (ECHODE_OK, None));
    assert_eq!(report.as_deref(), Some(&br#"{"parameters":[]}"#[..]));
}

#[test]
fn captures_return_to_the_worker_in_order_after_earlier_values() {
    let mut lane = DeviceLane::start().unwrap();
    let (order, seen) = mpsc::channel();
    let (set, set_done) = job();
    let early = order.clone();
    lane.report(
        set,
        snapshot(),
        Box::new(move || {
            early.send("set").unwrap();
            Ok(b"{}".to_vec())
        }),
    );
    let (commit, commit_done) = job();
    let device = Id::from_raw(0xC0);
    lane.capture(
        commit,
        device,
        Box::new(move || {
            order.send("capture").unwrap();
            Ok(b"bytes".to_vec())
        }),
    );
    assert!(lane.waiting());
    let returned = loop {
        if let Some(returned) = lane.take_returned() {
            break returned;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(!lane.waiting());
    assert_eq!(seen.try_iter().collect::<Vec<_>>(), ["set", "capture"]);
    assert_eq!(set_done.recv_timeout(WAIT).unwrap().0, ECHODE_OK);
    assert_eq!(returned.device, device);
    assert_eq!(returned.reply.as_deref(), Ok(&b"bytes"[..]));
    // Only the worker completes a capture, after recording it.
    assert!(commit_done.try_recv().is_err());
    finish(&returned.job, Err("recorded elsewhere".into()));
    let (status, message, _) = commit_done.recv_timeout(WAIT).unwrap();
    assert_eq!(
        (status, message.as_deref()),
        (ECHODE_ERR, Some("recorded elsewhere"))
    );
}

#[test]
fn closing_refuses_queued_requests_and_unrecorded_captures() {
    let mut lane = DeviceLane::start().unwrap();
    let (running, running_done) = job();
    let (round_trip, reached, release) = held(Ok(b"{}".to_vec()));
    lane.report(running, snapshot(), round_trip);
    reached.recv_timeout(WAIT).unwrap();
    let (queued, queued_done) = job();
    lane.report(queued, snapshot(), Box::new(|| Ok(b"{}".to_vec())));
    let (capture, capture_done) = job();
    lane.capture(
        capture,
        Id::from_raw(0xC1),
        Box::new(|| Ok(b"bytes".to_vec())),
    );
    lane.close();
    release.send(()).unwrap();
    lane.join();
    // The request that was already waiting finishes; the rest are refused.
    assert_eq!(running_done.recv_timeout(WAIT).unwrap().0, ECHODE_OK);
    for done in [queued_done, capture_done] {
        let (status, message, report) = done.recv_timeout(WAIT).unwrap();
        assert_eq!((status, message.as_deref()), (ECHODE_ERR, Some(STOPPED)));
        assert!(report.is_none());
    }
    let (late, late_done) = job();
    lane.report(late, snapshot(), Box::new(|| Ok(b"{}".to_vec())));
    assert_eq!(
        late_done.recv_timeout(WAIT).unwrap().1.as_deref(),
        Some(STOPPED)
    );
}

#[test]
fn a_failed_or_panicking_round_trip_completes_with_an_error() {
    let mut lane = DeviceLane::start().unwrap();
    let (failed, failed_done) = job();
    lane.report(
        failed,
        snapshot(),
        Box::new(|| Err("parameter changes timed out".into())),
    );
    let (panicked, panicked_done) = job();
    lane.report(panicked, snapshot(), Box::new(|| panic!("plugin protocol")));
    let (after, after_done) = job();
    lane.report(after, snapshot(), Box::new(|| Ok(b"{}".to_vec())));
    let message = |done: mpsc::Receiver<Completion>| done.recv_timeout(WAIT).unwrap().1;
    assert_eq!(
        message(failed_done).as_deref(),
        Some("parameter changes timed out")
    );
    assert_eq!(
        message(panicked_done).as_deref(),
        Some("device request panicked")
    );
    // The lane keeps serving later requests.
    assert_eq!(after_done.recv_timeout(WAIT).unwrap().0, ECHODE_OK);
}
