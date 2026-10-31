use std::sync::mpsc;
use std::time::Duration;

use super::*;

/// The view each test completes with: just the report it carried.
type View = Vec<u8>;
type Completion = Result<View, String>;

fn lane() -> DeviceLane<View> {
    DeviceLane::start(|_, report| report).unwrap()
}

fn ticket() -> (Ticket<View>, mpsc::Receiver<Completion>) {
    let (sender, done) = mpsc::channel();
    let ticket = Ticket::new(move |result| {
        let _ = sender.send(result);
    });
    (ticket, done)
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
    let mut lane = lane();
    let (first, done) = ticket();
    let (round_trip, reached, release) = held(Ok(br#"{"parameters":[]}"#.to_vec()));
    lane.report(first, View::new(), round_trip);
    reached.recv_timeout(WAIT).unwrap();
    // The worker keeps going: nothing is returned and nothing completed.
    assert!(lane.take_returned().is_none() && !lane.waiting());
    assert!(done.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(
        done.recv_timeout(WAIT).unwrap().as_deref(),
        Ok(&br#"{"parameters":[]}"#[..])
    );
}

#[test]
fn captures_return_to_the_worker_in_order_after_earlier_values() {
    let mut lane = lane();
    let (order, seen) = mpsc::channel();
    let (set, set_done) = ticket();
    let early = order.clone();
    lane.report(
        set,
        View::new(),
        Box::new(move || {
            early.send("set").unwrap();
            Ok(b"{}".to_vec())
        }),
    );
    let (commit, commit_done) = ticket();
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
    assert!(set_done.recv_timeout(WAIT).unwrap().is_ok());
    assert_eq!(returned.device, device);
    assert_eq!(returned.reply.as_deref(), Ok(&b"bytes"[..]));
    // Only the worker completes a capture, after recording it.
    assert!(commit_done.try_recv().is_err());
    returned.ticket.finish(Err("recorded elsewhere".into()));
    assert_eq!(
        commit_done.recv_timeout(WAIT).unwrap().unwrap_err(),
        "recorded elsewhere"
    );
}

#[test]
fn closing_refuses_queued_requests_and_unrecorded_captures() {
    let mut lane = lane();
    let (running, running_done) = ticket();
    let (round_trip, reached, release) = held(Ok(b"{}".to_vec()));
    lane.report(running, View::new(), round_trip);
    reached.recv_timeout(WAIT).unwrap();
    let (queued, queued_done) = ticket();
    lane.report(queued, View::new(), Box::new(|| Ok(b"{}".to_vec())));
    let (capture, capture_done) = ticket();
    lane.capture(
        capture,
        Id::from_raw(0xC1),
        Box::new(|| Ok(b"bytes".to_vec())),
    );
    lane.close();
    release.send(()).unwrap();
    lane.join();
    // The request that was already waiting finishes; the rest are refused.
    assert!(running_done.recv_timeout(WAIT).unwrap().is_ok());
    for done in [queued_done, capture_done] {
        assert_eq!(done.recv_timeout(WAIT).unwrap().unwrap_err(), STOPPED);
    }
    let (late, late_done) = ticket();
    lane.report(late, View::new(), Box::new(|| Ok(b"{}".to_vec())));
    assert_eq!(late_done.recv_timeout(WAIT).unwrap().unwrap_err(), STOPPED);
}

#[test]
fn a_failed_or_panicking_round_trip_completes_with_an_error() {
    let mut lane = lane();
    let (failed, failed_done) = ticket();
    lane.report(
        failed,
        View::new(),
        Box::new(|| Err("parameter changes timed out".into())),
    );
    let (panicked, panicked_done) = ticket();
    lane.report(
        panicked,
        View::new(),
        Box::new(|| panic!("plugin protocol")),
    );
    let (after, after_done) = ticket();
    lane.report(after, View::new(), Box::new(|| Ok(b"{}".to_vec())));
    assert_eq!(
        failed_done.recv_timeout(WAIT).unwrap().unwrap_err(),
        "parameter changes timed out"
    );
    assert_eq!(
        panicked_done.recv_timeout(WAIT).unwrap().unwrap_err(),
        "device request panicked"
    );
    // The lane keeps serving later requests.
    assert!(after_done.recv_timeout(WAIT).unwrap().is_ok());
}
