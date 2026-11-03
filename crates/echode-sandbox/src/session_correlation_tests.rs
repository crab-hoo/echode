use super::*;

#[test]
fn latency_is_retained_when_another_request_drains_the_socket() {
    let (parent, _helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let mut state = control.lock();
    state.accept(
        0,
        ControlMessage::PluginRestarted {
            latency_samples: u32::MAX,
        },
    );
    assert_eq!(control.latency_samples(), u32::MAX);
    assert_eq!(
        control.take_activity().unwrap().latency_samples,
        Some(u32::MAX)
    );
    // An unsolicited late reply must not replace newer metadata.
    state.accept(
        99,
        ControlMessage::PluginRestarted {
            latency_samples: 64,
        },
    );
    assert_eq!(control.latency_samples(), u32::MAX);
    state.accept(0, ControlMessage::PluginRestarted { latency_samples: 0 });
    assert_eq!(control.latency_samples(), 0);
    assert_eq!(control.take_activity().unwrap().latency_samples, Some(0));
}

#[test]
fn contract_control_replies_are_correlated_across_timeouts() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    // A deterministic timeout, without sleeping or depending on scheduling.
    let mut state = control.lock();
    let old = state.request(ControlMessage::SaveRequest).unwrap();
    assert!(matches!(
        state.wait_reply(old, Duration::ZERO, |_| true),
        Err(SessionError::Timeout)
    ));
    let (wire_id, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
    assert_eq!(wire_id, old);
    assert!(matches!(request, ControlMessage::SaveRequest));
    helper
        .send_tagged(old, &ControlMessage::State(vec![64]))
        .unwrap();
    helper.send(&ControlMessage::Dirty).unwrap();
    state.drain().unwrap();
    assert!(
        state.inbox.iter().all(|(id, _)| *id == 0),
        "late replies must be discarded"
    );
    assert!(
        state.applied.lock().unwrap().is_none(),
        "late save changed recorded state"
    );
    drop(state);

    let answer = std::thread::spawn(move || {
        let (restore, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert!(restore > old);
        assert!(matches!(request, ControlMessage::Restore(bytes) if bytes == [128]));
        // Same error type as the restore's errors, but for the expired save.
        helper
            .send_tagged(old, &ControlMessage::StateRejected("old timeout".into()))
            .unwrap();
        helper
            .send_tagged(restore, &ControlMessage::Restored)
            .unwrap();
        let (save, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert!(save > restore);
        assert!(matches!(request, ControlMessage::SaveRequest));
        helper
            .send_tagged(old, &ControlMessage::State(vec![64]))
            .unwrap();
        helper
            .send_tagged(save, &ControlMessage::State(vec![128]))
            .unwrap();
        helper
    });
    control.restore_state(&[128]).unwrap();
    assert_eq!(control.capture_state().unwrap(), [128]);
    assert_eq!(control.applied_state().unwrap(), [128]);
    assert!(matches!(
        control.lock().poll_notice().unwrap(),
        Some(PluginNotice::Dirty)
    ));
    drop(answer.join().unwrap());
}

#[test]
fn background_and_foreground_saves_keep_distinct_replies() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    control.lock().request_save().unwrap();
    assert!(
        control.lock().request_save().is_err(),
        "two background saves must not replace an id"
    );
    let answer = std::thread::spawn(move || {
        let (background, _) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        let (foreground, _) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert_ne!(background, foreground);
        helper
            .send_tagged(background, &ControlMessage::State(vec![1]))
            .unwrap();
        helper
            .send_tagged(foreground, &ControlMessage::State(vec![2]))
            .unwrap();
        helper
    });
    assert_eq!(
        control.lock().save_state().unwrap(),
        [2],
        "foreground save took background reply"
    );
    assert_eq!(control.lock().poll_saved().unwrap().unwrap().unwrap(), [1]);
    assert_eq!(
        control.applied_state().unwrap(),
        [2],
        "older background save replaced newer state"
    );
    assert!(control.lock().poll_saved().unwrap().is_none());
    drop(answer.join().unwrap());
}

#[test]
fn expired_parameter_and_window_replies_cannot_answer_new_calls() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let mut state = control.lock();
    let old_params = state.request(ControlMessage::ParamsRequest).unwrap();
    assert!(matches!(
        state.wait_reply(old_params, Duration::ZERO, |_| true),
        Err(SessionError::Timeout)
    ));
    let old_window = state
        .request(ControlMessage::GuiShow("old".into()))
        .unwrap();
    assert!(matches!(
        state.wait_reply(old_window, Duration::ZERO, |_| true),
        Err(SessionError::Timeout)
    ));
    drop(state);
    let answer = std::thread::spawn(move || {
        helper.recv_tagged(Duration::from_secs(2)).unwrap();
        helper.recv_tagged(Duration::from_secs(2)).unwrap();
        let (params, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert!(matches!(request, ControlMessage::ParamsRequest));
        helper
            .send_tagged(
                old_params,
                &ControlMessage::ParamsRejected("expired".into()),
            )
            .unwrap();
        helper
            .send_tagged(params, &ControlMessage::Params(vec![]))
            .unwrap();
        let (window, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert!(matches!(request, ControlMessage::GuiClose));
        helper
            .send_tagged(old_window, &ControlMessage::GuiRejected("expired".into()))
            .unwrap();
        helper
            .send_tagged(window, &ControlMessage::GuiClosedReply)
            .unwrap();
        helper
    });
    assert!(control.parameters().unwrap().is_empty());
    control.close_gui().unwrap();
    drop(answer.join().unwrap());
}
