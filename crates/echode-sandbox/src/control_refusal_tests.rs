use super::*;
use crate::{RefusalDecodeError, RefusalField};

fn receive(typ: u32, payload: &[u8]) -> io::Result<(u64, ControlMessage)> {
    let (sender, mut receiver) = Socket::pair()?;
    send_bytes(sender.as_raw_fd(), &frame_tagged(73, typ, payload), None)?;
    receiver.recv_tagged(Duration::from_secs(2))
}

fn malformed_cases() -> Vec<(Vec<u8>, RefusalDecodeError, &'static str)> {
    use RefusalDecodeError::*;
    use RefusalField::*;
    let mut cases = vec![(vec![], MissingKind, "refusal has no kind")];
    for kind in [0].into_iter().chain(11..=u8::MAX) {
        cases.push((vec![kind], UnknownKind(kind), "refusal has an unknown kind"));
    }
    for kind in [1, 2, 4, 6] {
        for actual in [1, 3] {
            let mut payload = vec![kind];
            payload.extend(std::iter::repeat_n(0, actual));
            cases.push((
                payload,
                TrailingBytes { kind, actual },
                "refusal has trailing bytes",
            ));
        }
    }
    for (kind, field, expected, diagnostic) in [
        (3, MainCall, 8, "refusal call number is the wrong size"),
        (5, MainCall, 8, "refusal call number is the wrong size"),
        (9, ParameterId, 4, "refusal parameter id is the wrong size"),
        (10, MainReplies, 2, "refusal reply kinds are the wrong size"),
    ] {
        for actual in (0..=12).filter(|size| *size != expected) {
            let mut payload = vec![kind];
            payload.extend(std::iter::repeat_n(0, actual));
            cases.push((
                payload,
                WrongSize {
                    field,
                    expected,
                    actual,
                },
                diagnostic,
            ));
        }
    }
    for code in 6..=u8::MAX {
        cases.push((
            vec![10, code, 0],
            UnknownExpectedReply(code),
            "refusal expected reply kind is unknown",
        ));
        cases.push((
            vec![10, 0, code],
            UnknownActualReply(code),
            "refusal actual reply kind is unknown",
        ));
    }
    cases.push((vec![7], MissingPluginKind, "plugin failure is malformed"));
    for kind in 0..=24 {
        let expected = if matches!(kind, 18 | 19) { 5 } else { 0 };
        for actual in (0..=8).filter(|size| *size != expected) {
            let mut payload = vec![7, kind];
            payload.extend(std::iter::repeat_n(0, actual));
            cases.push((
                payload,
                WrongSize {
                    field: PluginPayload { kind },
                    expected,
                    actual,
                },
                "plugin failure is malformed",
            ));
        }
    }
    for kind in 25..=u8::MAX {
        cases.push((
            vec![7, kind],
            UnknownPluginKind(kind),
            "plugin failure is malformed",
        ));
    }
    for kind in [18, 19] {
        for side in 2..=u8::MAX {
            cases.push((
                vec![7, kind, side, 0, 0, 0, 0],
                InvalidPortSide { kind, side },
                "plugin failure is malformed",
            ));
        }
    }
    cases
}

#[test]
fn malformed_refusal_structure_retains_causes_through_every_socket_reply() {
    for typ in [
        PORTS_REJECTED,
        STATE_REJECTED,
        PARAMS_REJECTED,
        GUI_REJECTED,
    ] {
        for (payload, expected, diagnostic) in malformed_cases() {
            let error = receive(typ, &payload).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert_eq!(error.to_string(), diagnostic, "{typ}: {payload:?}");
            let cause = error
                .get_ref()
                .and_then(|error| error.downcast_ref::<RefusalDecodeError>())
                .expect("malformed refusal must retain its typed structural cause");
            assert_eq!(cause, &expected, "{typ}: {payload:?}");
            assert!(std::error::Error::source(cause).is_none());
        }
    }
}

#[test]
fn valid_refusals_keep_their_values_through_every_socket_reply() {
    use crate::{AudioPort, MainReplyKind, Pending, PluginFailure, Refusal};
    let refusals = [
        Refusal::Busy,
        Refusal::NoPlugin,
        Refusal::TimedOut(Pending::MainCall(u64::MAX)),
        Refusal::Stopped(Pending::MainCall(0)),
        Refusal::TimedOut(Pending::ParameterChanges),
        Refusal::Stopped(Pending::ParameterChanges),
        Refusal::InvalidParameterId(u32::MAX),
        Refusal::UnexpectedMainReply {
            expected: MainReplyKind::State,
            actual: MainReplyKind::Window,
        },
        Refusal::Plugin(PluginFailure::NoState),
        Refusal::Plugin(PluginFailure::PortUndescribed(AudioPort {
            input: false,
            index: 0,
        })),
        Refusal::Plugin(PluginFailure::PortWithoutChannels(AudioPort {
            input: true,
            index: u32::MAX,
        })),
    ];
    for typ in [
        PORTS_REJECTED,
        STATE_REJECTED,
        PARAMS_REJECTED,
        GUI_REJECTED,
    ] {
        for refusal in &refusals {
            let payload = crate::refusal::write_refusal(refusal);
            let (id, message) = receive(typ, &payload).unwrap();
            assert_eq!(id, 73);
            let received = match message {
                ControlMessage::PortsRejected(refusal)
                | ControlMessage::StateRejected(refusal)
                | ControlMessage::ParamsRejected(refusal)
                | ControlMessage::GuiRejected(refusal) => refusal,
                other => panic!("expected a refusal, got {other:?}"),
            };
            assert_eq!(&received, refusal);
            assert_eq!(crate::refusal::write_refusal(&received), payload);
        }
    }
}
