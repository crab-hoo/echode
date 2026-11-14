use super::*;
use crate::{MainReplyKind, Pending, PluginFailure};

fn requested(helper: &mut Socket) -> u64 {
    let Some((id, ControlMessage::SaveRequest)) = helper.try_recv_tagged().unwrap() else {
        panic!("expected a background capture request");
    };
    id
}

#[test]
fn timed_out_captures_retry_with_fresh_ids_and_warn_once_until_success() {
    let capture = echode_log::testing::capture();
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    let mut now = Instant::now();
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    let mut previous = requested(&mut helper);
    for refusal in [
        Refusal::TimedOut(Pending::MainCall(7)),
        Refusal::Busy,
        Refusal::TimedOut(Pending::ParameterChanges),
    ] {
        helper
            .send_tagged(previous, &ControlMessage::StateRejected(refusal))
            .unwrap();
        assert!(control.poll_state_save_at(now).is_none());
        assert!(control.lock().dirty_again);
        assert!(control.lock().pending.is_empty());
        assert!(
            control
                .poll_state_save_at(now + SAVE_RETRY - Duration::from_nanos(1))
                .is_none()
        );
        assert!(helper.try_recv_tagged().unwrap().is_none());
        now += SAVE_RETRY;
        assert!(control.poll_state_save_at(now).is_none());
        let next = requested(&mut helper);
        assert!(next > previous);
        // A late result or refusal for an abandoned capture cannot satisfy this one.
        helper
            .send_tagged(previous, &ControlMessage::State(vec![1]))
            .unwrap();
        helper
            .send_tagged(previous, &ControlMessage::StateRejected(Refusal::Busy))
            .unwrap();
        assert!(control.poll_state_save_at(now).is_none());
        assert!(control.applied_state().is_none());
        previous = next;
    }
    let records = capture.on_this_thread();
    assert_eq!(records.len(), 1, "one warning per failure episode");
    assert_eq!(records[0]["level"], "warn");
    assert_eq!(records[0]["target"], "echode_sandbox::session");
    assert_eq!(
        records[0]["message"],
        "background plugin state capture failed"
    );
    assert_eq!(records[0]["fields"]["retry"], true);
    assert_eq!(records[0]["fields"]["request_id"], 1);
    assert_eq!(
        records[0]["fields"]["cause"],
        "Refused(TimedOut(MainCall(7)))"
    );

    helper
        .send_tagged(previous, &ControlMessage::State(vec![42]))
        .unwrap();
    assert_eq!(control.poll_state_save_at(now), Some(vec![42]));
    assert_eq!(control.applied_state().unwrap().bytes(), [42]);
    assert!(control.poll_state_save_at(now + SAVE_RETRY).is_none());
    assert!(helper.try_recv_tagged().unwrap().is_none());

    // A new episode after a successful capture is visible in the product log.
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    let id = requested(&mut helper);
    helper
        .send_tagged(id, &ControlMessage::StateRejected(Refusal::Busy))
        .unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    assert_eq!(capture.on_this_thread().len(), 2);
}

#[test]
fn terminal_refusals_consume_only_the_change_that_started_the_capture() {
    let capture = echode_log::testing::capture();
    let refusals = [
        Refusal::NoPlugin,
        Refusal::Stopped(Pending::MainCall(7)),
        Refusal::Plugin(PluginFailure::NoState),
        Refusal::Plugin(PluginFailure::NoStateSave),
        Refusal::Plugin(PluginFailure::SaveRefused),
        Refusal::UnexpectedMainReply {
            expected: MainReplyKind::State,
            actual: MainReplyKind::Done,
        },
    ];
    for refusal in refusals {
        for newer_dirty in [false, true] {
            let (parent, mut helper) = Socket::pair().unwrap();
            let mut control = PluginControl::new(parent);
            let now = Instant::now();
            helper.send(&ControlMessage::Dirty).unwrap();
            assert!(control.poll_state_save_at(now).is_none());
            let id = requested(&mut helper);
            if newer_dirty {
                helper.send(&ControlMessage::Dirty).unwrap();
            }
            helper
                .send_tagged(id, &ControlMessage::StateRejected(refusal.clone()))
                .unwrap();
            assert!(control.poll_state_save_at(now).is_none());
            assert_eq!(control.lock().dirty_again, newer_dirty);
            assert!(control.lock().pending.is_empty());
            assert!(control.poll_state_save_at(now).is_none());
            assert!(helper.try_recv_tagged().unwrap().is_none());
            let later = now + SAVE_RETRY;
            if !newer_dirty {
                assert!(control.poll_state_save_at(later).is_none());
                assert!(helper.try_recv_tagged().unwrap().is_none(), "{refusal:?}");
                helper.send(&ControlMessage::Dirty).unwrap();
            }
            assert!(control.poll_state_save_at(later).is_none());
            let next = requested(&mut helper);
            assert!(next > id);
            helper
                .send_tagged(next, &ControlMessage::State(vec![2]))
                .unwrap();
            assert_eq!(control.poll_state_save_at(later), Some(vec![2]));
        }
    }
    let records = capture.on_this_thread();
    assert_eq!(records.len(), 12);
    for pair in records.chunks_exact(2) {
        assert_eq!(pair[0]["fields"]["retry"], false);
        assert_eq!(pair[1]["fields"]["retry"], true);
    }
}

#[test]
fn a_dirty_notice_during_a_successful_capture_still_gets_its_own_capture() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    let now = Instant::now();
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    let first = requested(&mut helper);
    helper.send(&ControlMessage::Dirty).unwrap();
    helper
        .send_tagged(first, &ControlMessage::State(vec![1]))
        .unwrap();
    assert_eq!(control.poll_state_save_at(now), Some(vec![1]));
    assert!(control.poll_state_save_at(now).is_none());
    let second = requested(&mut helper);
    assert!(second > first);
    helper
        .send_tagged(second, &ControlMessage::State(vec![2]))
        .unwrap();
    assert_eq!(control.poll_state_save_at(now), Some(vec![2]));
    assert_eq!(control.applied_state().unwrap().bytes(), [2]);
    assert!(control.poll_state_save_at(now).is_none());
    assert!(helper.try_recv_tagged().unwrap().is_none());
}

#[test]
fn a_disconnected_capture_keeps_the_change_without_a_hot_retry_or_log_loop() {
    let capture = echode_log::testing::capture();
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    let now = Instant::now();
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save_at(now).is_none());
    let first = requested(&mut helper);
    drop(helper);
    assert!(control.poll_state_save_at(now).is_none());
    assert!(control.lock().dirty_again);
    assert!(control.lock().pending.is_empty());
    for turn in 0..50 {
        assert!(
            control
                .poll_state_save_at(now + Duration::from_millis(turn * 20))
                .is_none()
        );
        assert_eq!(control.lock().next_request, first);
    }
    for turn in 1..=3 {
        assert!(
            control
                .poll_state_save_at(now + SAVE_RETRY * turn)
                .is_none()
        );
        assert!(control.lock().dirty_again);
        assert!(control.lock().pending.is_empty());
    }
    assert_eq!(capture.on_this_thread().len(), 1);
}
