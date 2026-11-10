use super::*;
use crate::{GuiReport, ParamChange, ParamInfo, ParamText, ParamValue, Pending};

fn receive(helper: &mut Socket) -> (u64, ControlMessage) {
    let (id, message) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
    assert_ne!(id, 0, "operations must use correlated requests");
    (id, message)
}

#[test]
fn parameter_operation_retains_invalid_utf8_from_the_helper() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let (parent, mut raw) = UnixStream::pair().unwrap();
    let control = PluginControl::new(Socket::from_owned(parent.into()).unwrap());
    let mut helper = Socket::from_owned(raw.try_clone().unwrap().into()).unwrap();
    let answer = std::thread::spawn(move || {
        let (id, request) = receive(&mut helper);
        assert!(matches!(request, ControlMessage::ParamsRequest));
        let mut body = crate::params::write_infos(&[ParamInfo {
            id: 7,
            flags: 0,
            name: "Gain".into(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            value: 0.25,
            text: "ab_".into(),
        }]);
        *body.last_mut().unwrap() = 0xff;
        // A raw Params (21) frame deliberately bypasses the typed sender.
        let mut frame = (12 + body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&21u32.to_le_bytes());
        frame.extend_from_slice(&id.to_le_bytes());
        frame.extend_from_slice(&body);
        raw.write_all(&frame).unwrap();
    });
    let error = control.parameters().unwrap_err();
    answer.join().unwrap();
    let SessionError::Io(error) = error else {
        panic!("invalid parameter text must remain a typed I/O cause: {error:?}");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "parameter text is not UTF-8");
    let cause = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<crate::ControlTextError>())
        .expect("the parameter operation must retain the control text error");
    let crate::ControlTextError::Parameter(utf8) = cause else {
        panic!("the cause must identify parameter text");
    };
    assert_eq!(utf8.valid_up_to(), 2);
    assert_eq!(utf8.error_len(), Some(1));
}

#[test]
fn parameter_operation_preserves_malformed_launch_refusal_causes() {
    use crate::{LaunchDecodeError, LaunchField};
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let invalid_text = vec![0xff];
    for (payload, expected) in [
        (
            vec![8],
            LaunchDecodeError::Truncated {
                field: LaunchField::Kind,
                expected: 1,
                actual: 0,
            },
        ),
        (vec![8, 2, 2], LaunchDecodeError::InvalidErrnoFlag(2)),
        (
            vec![8, 0, 0xff],
            LaunchDecodeError::Diagnostic(std::str::from_utf8(&invalid_text).unwrap_err()),
        ),
    ] {
        let (parent, mut raw) = UnixStream::pair().unwrap();
        let control = PluginControl::new(Socket::from_owned(parent.into()).unwrap());
        let mut helper = Socket::from_owned(raw.try_clone().unwrap().into()).unwrap();
        let answer = std::thread::spawn(move || {
            let (id, request) = receive(&mut helper);
            assert!(matches!(request, ControlMessage::ParamsRequest));
            // ParamsRejected (24), with a raw payload bypassing the typed sender.
            let mut frame = (12 + payload.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&24u32.to_le_bytes());
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&payload);
            raw.write_all(&frame).unwrap();
        });
        let error = control.parameters().unwrap_err();
        answer.join().unwrap();
        let SessionError::Io(error) = error else {
            panic!("malformed launch refusal must remain an I/O cause: {error:?}");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "launch failure is malformed");
        assert_eq!(
            error.get_ref().unwrap().downcast_ref::<LaunchDecodeError>(),
            Some(&expected)
        );
    }
}

#[test]
fn public_control_operations_preserve_payloads_and_typed_replies() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let info = ParamInfo {
        id: 19,
        flags: 0,
        name: "Level — 音量".into(),
        module: "Main".into(),
        min: -2.0,
        max: 2.0,
        default: 0.0,
        value: 0.25,
        text: "25%".into(),
    };
    let expected = info.clone();
    let answer = std::thread::spawn(move || {
        let (params, request) = receive(&mut helper);
        assert!(matches!(request, ControlMessage::ParamsRequest));
        helper
            .send_tagged(params, &ControlMessage::Params(vec![info]))
            .unwrap();
        let (set, request) = receive(&mut helper);
        assert!(set > params);
        assert!(matches!(request, ControlMessage::SetParams(changes)
            if changes == [ParamChange { id: 19, value: -0.5 }]));
        helper
            .send_tagged(
                set,
                &ControlMessage::ParamsApplied(vec![ParamValue {
                    id: 19,
                    value: -0.5,
                    text: "-50%".into(),
                }]),
            )
            .unwrap();
        let (parse, request) = receive(&mut helper);
        assert!(parse > set);
        assert!(
            matches!(request, ControlMessage::ParseParam { id: 19, text }
            if text == "−50%")
        );
        helper
            .send_tagged(parse, &ControlMessage::ParamParsed(ParamText::Value(-0.5)))
            .unwrap();
        let (restore, request) = receive(&mut helper);
        assert!(restore > parse);
        assert!(matches!(request, ControlMessage::Restore(bytes) if bytes == [0, 128, 255]));
        helper
            .send_tagged(restore, &ControlMessage::Restored)
            .unwrap();
        let (background, request) = receive(&mut helper);
        assert!(matches!(request, ControlMessage::SaveRequest));
        assert!(helper.try_recv_tagged().unwrap().is_none());
        helper
            .send_tagged(background, &ControlMessage::State(vec![1]))
            .unwrap();
        let (capture, request) = receive(&mut helper);
        assert!(capture > background);
        assert!(matches!(request, ControlMessage::SaveRequest));
        helper
            .send_tagged(capture, &ControlMessage::State(vec![2, 0, 255]))
            .unwrap();
        let (show, request) = receive(&mut helper);
        assert!(show > capture);
        assert!(matches!(request, ControlMessage::GuiShow(title) if title == "Pad — 音源"));
        helper
            .send_tagged(
                show,
                &ControlMessage::GuiShown(GuiReport {
                    created: true,
                    visible: false,
                    key: false,
                }),
            )
            .unwrap();
        let (close, request) = receive(&mut helper);
        assert!(close > show);
        assert!(matches!(request, ControlMessage::GuiClose));
        helper
            .send_tagged(close, &ControlMessage::GuiClosedReply)
            .unwrap();
        helper
    });

    assert_eq!(control.parameters().unwrap(), [expected]);
    assert_eq!(
        control
            .set_parameters(&[ParamChange {
                id: 19,
                value: -0.5
            }])
            .unwrap(),
        [ParamValue {
            id: 19,
            value: -0.5,
            text: "-50%".into()
        }]
    );
    assert_eq!(
        control.parse_parameter(19, "−50%").unwrap(),
        ParamText::Value(-0.5)
    );
    control.restore_state(&[0, 128, 255]).unwrap();
    assert_eq!(control.applied_state().unwrap(), [0, 128, 255]);
    control.lock().request_save().unwrap();
    assert_eq!(control.capture_state().unwrap(), [2, 0, 255]);
    assert_eq!(control.applied_state().unwrap(), [2, 0, 255]);
    assert!(control.lock().poll_saved().unwrap().is_none());
    let report = control.show_gui("Pad — 音源").unwrap();
    assert!(report.created && !report.visible && !report.key);
    assert_eq!(control.take_activity().unwrap().window_state, Some(true));
    control.close_gui().unwrap();
    assert_eq!(control.take_activity().unwrap().window_state, Some(false));
    drop(answer.join().unwrap());
}

/// One refusal per operation below, covering every kind.
fn refusals() -> [Refusal; 7] {
    [
        Refusal::Busy,
        Refusal::TimedOut(Pending::ParameterChanges),
        Refusal::Plugin(crate::PluginFailure::NoParameterFlush),
        Refusal::TimedOut(Pending::MainCall(5)),
        Refusal::Stopped(Pending::ParameterChanges),
        Refusal::Stopped(Pending::MainCall(6)),
        Refusal::NoPlugin,
    ]
}

#[test]
fn public_control_rejections_preserve_the_last_applied_state() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    let answer = std::thread::spawn(move || {
        let (id, request) = receive(&mut helper);
        assert!(matches!(request, ControlMessage::Restore(bytes) if bytes == [42]));
        helper.send_tagged(id, &ControlMessage::Restored).unwrap();
        for (operation, reason) in refusals().into_iter().enumerate() {
            let (id, request) = receive(&mut helper);
            let reply = match (operation, request) {
                (0, ControlMessage::ParamsRequest)
                | (1, ControlMessage::SetParams(_))
                | (2, ControlMessage::ParseParam { .. }) => ControlMessage::ParamsRejected(reason),
                (3, ControlMessage::Restore(_)) | (4, ControlMessage::SaveRequest) => {
                    ControlMessage::StateRejected(reason)
                }
                (5, ControlMessage::GuiShow(_)) | (6, ControlMessage::GuiClose) => {
                    ControlMessage::GuiRejected(reason)
                }
                (_, request) => panic!("unexpected operation {operation}: {request:?}"),
            };
            helper.send_tagged(id, &reply).unwrap();
        }
        helper
    });
    control.restore_state(&[42]).unwrap();
    let errors = [
        control.parameters().unwrap_err(),
        control
            .set_parameters(&[ParamChange { id: 1, value: 0.5 }])
            .unwrap_err(),
        control.parse_parameter(1, "half").unwrap_err(),
        control.restore_state(&[99]).unwrap_err(),
        control.capture_state().unwrap_err(),
        control.show_gui("refused").unwrap_err(),
        control.close_gui().unwrap_err(),
    ];
    // Each kind arrives as the helper sent it, not as text to match.
    for ((operation, error), expected) in errors.into_iter().enumerate().zip(refusals()) {
        assert!(
            matches!(&error, SessionError::Refused(refusal) if *refusal == expected),
            "operation {operation}: {error:?}"
        );
        assert_eq!(error.to_string(), expected.to_string());
    }
    assert_eq!(control.applied_state().unwrap(), [42]);
    assert!(control.take_activity().is_none());
    drop(answer.join().unwrap());
}

#[test]
fn invalid_values_never_reach_the_socket_and_local_reads_do_not_wait_on_it() {
    let (parent, mut helper) = Socket::pair().unwrap();
    let control = PluginControl::new(parent);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = control
            .set_parameters(&[ParamChange { id: 1, value }])
            .unwrap_err();
        assert!(matches!(error, SessionError::NonFiniteParameter));
    }
    assert!(helper.try_recv_tagged().unwrap().is_none());
    let mut background = control.clone();
    let _held = control.lock();
    assert!(background.poll_state_save().is_none());
    assert!(background.applied_state().is_none());
    assert!(background.take_activity().is_none());
}
