//! A foreground capture must not consume the background change it takes over
//! when that capture fails. These tests use the product control socket only.
use super::*;
use crate::{Pending, PluginFailure};

fn requested(helper: &mut Socket) -> u64 {
    let (id, request) = helper.recv_tagged(Duration::from_secs(5)).unwrap();
    assert!(matches!(request, ControlMessage::SaveRequest));
    id
}

fn assert_background_recovers(control: &mut PluginControl, helper: &mut Socket, previous: u64) {
    let state = control.lock();
    assert!(
        state.dirty_again,
        "the failed foreground capture consumed the background change"
    );
    assert!(state.pending.is_empty());
    assert!(state.save_requested.is_none());
    let after = state.save_retry_after.expect("background retry deadline");
    drop(state);
    assert!(
        control
            .poll_state_save_at(after - Duration::from_nanos(1))
            .is_none()
    );
    assert!(helper.try_recv_tagged().unwrap().is_none());
    assert!(control.poll_state_save_at(after).is_none());
    let next = requested(helper);
    assert!(next > previous);
    // Neither a late state nor a late refusal can complete the new capture.
    helper
        .send_tagged(previous, &ControlMessage::State(vec![1]))
        .unwrap();
    helper
        .send_tagged(previous, &ControlMessage::StateRejected(Refusal::Busy))
        .unwrap();
    assert!(control.poll_state_save_at(after).is_none());
    assert_eq!(control.applied_state().unwrap().bytes(), [9]);
    helper
        .send_tagged(next, &ControlMessage::State(vec![42]))
        .unwrap();
    assert_eq!(control.poll_state_save_at(after), Some(vec![42]));
    assert_eq!(control.applied_state().unwrap().bytes(), [42]);
    assert!(control.poll_state_save_at(after + SAVE_RETRY).is_none());
    assert!(helper.try_recv_tagged().unwrap().is_none());
}

#[test]
fn audit_dia02_dia04_a_failed_foreground_capture_returns_its_background_change() {
    for background in [
        ControlMessage::State(vec![7]),
        ControlMessage::StateRejected(Refusal::Busy),
        ControlMessage::StateRejected(Refusal::TimedOut(Pending::MainCall(7))),
    ] {
        for refusal in [Refusal::Busy, Refusal::TimedOut(Pending::MainCall(8))] {
            let capture = echode_log::testing::capture();
            let before = capture.on_this_thread().len();
            let (parent, mut helper) = Socket::pair().unwrap();
            let mut control = PluginControl::new(parent);
            control
                .lock()
                .remember(0, AppliedState::new([9].as_slice()));
            helper.send(&ControlMessage::Dirty).unwrap();
            assert!(control.poll_state_save().is_none());
            let background_id = requested(&mut helper);
            helper.send_tagged(background_id, &background).unwrap();
            let expected = refusal.clone();
            let answer = std::thread::spawn(move || {
                let id = requested(&mut helper);
                assert!(id > background_id);
                helper
                    .send_tagged(id, &ControlMessage::StateRejected(refusal))
                    .unwrap();
                (helper, id)
            });
            let result = control.capture_state();
            let (mut helper, id) = answer.join().unwrap();
            assert!(matches!(result, Err(SessionError::Refused(reason)) if reason == expected));
            assert_background_recovers(&mut control, &mut helper, id);
            let records = capture.on_this_thread();
            let records = &records[before..];
            assert_eq!(records.len(), 1);
            assert_eq!(
                records[0]["message"],
                "background plugin state capture failed"
            );
            assert_eq!(records[0]["fields"]["request_id"], id);
            assert_eq!(records[0]["fields"]["retry"], true);
            assert_eq!(
                records[0]["fields"]["cause"],
                format!("Refused({expected:?})")
            );
        }
    }
}

#[test]
fn audit_dia02_dia04_waiting_for_a_background_capture_can_time_out_without_losing_it() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let mut control = PluginControl::new(parent);
    control
        .lock()
        .remember(0, AppliedState::new([9].as_slice()));
    helper.send(&ControlMessage::Dirty).unwrap();
    assert!(control.poll_state_save().is_none());
    let id = requested(&mut helper);
    // Withhold the background reply until the foreground wait actually expires.
    assert!(matches!(
        control.capture_state(),
        Err(SessionError::Timeout)
    ));
    assert_background_recovers(&mut control, &mut helper, id);
}

#[test]
fn a_terminal_foreground_refusal_preserves_only_newer_background_changes() {
    for newer_dirty in [false, true] {
        let (parent, mut helper) = Socket::pair().unwrap();
        let mut control = PluginControl::new(parent);
        helper.send(&ControlMessage::Dirty).unwrap();
        assert!(control.poll_state_save().is_none());
        let background = requested(&mut helper);
        helper
            .send_tagged(background, &ControlMessage::StateRejected(Refusal::Busy))
            .unwrap();
        let answer = std::thread::spawn(move || {
            let id = requested(&mut helper);
            if newer_dirty {
                helper.send(&ControlMessage::Dirty).unwrap();
            }
            helper
                .send_tagged(
                    id,
                    &ControlMessage::StateRejected(Refusal::Plugin(PluginFailure::SaveRefused)),
                )
                .unwrap();
            helper
        });
        assert!(matches!(
            control.capture_state(),
            Err(SessionError::Refused(Refusal::Plugin(
                PluginFailure::SaveRefused
            )))
        ));
        let mut helper = answer.join().unwrap();
        // The newer notification may still be queued; the poller must collect it.
        assert!(control.poll_state_save().is_none());
        assert_eq!(control.lock().dirty_again, newer_dirty);
        assert!(helper.try_recv_tagged().unwrap().is_none());
    }
}

#[test]
fn a_foreground_capture_without_a_background_request_does_not_schedule_a_retry() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let answer = std::thread::spawn(move || {
        let id = requested(&mut helper);
        helper
            .send_tagged(id, &ControlMessage::StateRejected(Refusal::Busy))
            .unwrap();
        helper
    });
    assert!(matches!(
        control.capture_state(),
        Err(SessionError::Refused(Refusal::Busy))
    ));
    let mut helper = answer.join().unwrap();
    assert!(!control.lock().dirty_again);
    assert!(control.lock().save_retry_after.is_none());
    assert!(helper.try_recv_tagged().unwrap().is_none());
}
