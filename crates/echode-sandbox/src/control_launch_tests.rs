use super::*;
use crate::{LaunchDecodeError, LaunchFailure, LaunchFailureKind, LaunchField};
use std::str::Utf8Error;

const REJECTIONS: [u32; 4] = [
    PORTS_REJECTED,
    STATE_REJECTED,
    PARAMS_REJECTED,
    GUI_REJECTED,
];

fn receive(typ: u32, payload: &[u8]) -> io::Result<(u64, ControlMessage)> {
    let (sender, mut receiver) = Socket::pair()?;
    // A launch refusal (8) wraps the deliberately malformed launch payload.
    let mut body = vec![8];
    body.extend_from_slice(payload);
    send_bytes(sender.as_raw_fd(), &frame_tagged(73, typ, &body), None)?;
    receiver.recv_tagged(Duration::from_secs(2))
}

#[test]
fn invalid_launch_diagnostic_retains_utf8_cause_through_the_socket() {
    for typ in REJECTIONS {
        for kind in launch_kinds() {
            for text in [b"ab\xff!", b"ab\xe3\x81"] {
                let mut payload = LaunchFailure {
                    kind,
                    diagnostic: String::new(),
                }
                .encode();
                payload.extend_from_slice(text);
                let error = receive(typ, &payload).unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidData);
                assert_eq!(error.to_string(), "launch failure is malformed");
                let cause = error
                    .get_ref()
                    .and_then(|error| error.source())
                    .and_then(|error| error.downcast_ref::<Utf8Error>())
                    .expect("invalid launch diagnostic must retain the UTF-8 decoding cause");
                assert_eq!(
                    error.get_ref().unwrap().downcast_ref::<LaunchDecodeError>(),
                    Some(&LaunchDecodeError::Diagnostic(*cause))
                );
                assert_eq!(cause, &std::str::from_utf8(text).unwrap_err());
                assert_eq!(cause.valid_up_to(), 2);
            }
        }
    }
}

fn launch_kinds() -> [LaunchFailureKind; 5] {
    use LaunchFailureKind::*;
    [
        Library,
        InvalidNulPath { position: u64::MAX },
        EntryIo { raw_os_error: None },
        EntryIo {
            raw_os_error: Some(i32::MIN),
        },
        IncompatibleClapVersion {
            plugin: [u32::MAX, 0, 1],
            host: [1, 2, 3],
        },
    ]
}

#[test]
fn malformed_launch_metadata_keeps_field_sizes_and_invalid_codes() {
    use LaunchDecodeError::*;
    use LaunchField::*;
    let mut cases = Vec::new();
    for (field, prefix, expected) in [
        (Kind, vec![], 1),
        (NulPosition, vec![1], 8),
        (ErrnoFlag, vec![2], 1),
        (Errno, vec![2, 1], 4),
        (PluginVersion, vec![5], 12),
        (HostVersion, vec![5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 12),
    ] {
        for actual in 0..expected {
            let mut payload = prefix.clone();
            payload.extend(std::iter::repeat_n(0, actual));
            cases.push((
                payload,
                Truncated {
                    field,
                    expected,
                    actual,
                },
            ));
        }
    }
    for code in 27..=u8::MAX {
        cases.push((vec![code], UnknownKind(code)));
    }
    for flag in 2..=u8::MAX {
        cases.push((vec![2, flag], InvalidErrnoFlag(flag)));
    }
    for typ in REJECTIONS {
        for (payload, expected) in &cases {
            let error = receive(typ, payload).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "launch failure is malformed");
            let cause = error.get_ref().unwrap();
            assert_eq!(
                cause.downcast_ref::<LaunchDecodeError>(),
                Some(expected),
                "{typ}: {payload:?}"
            );
            assert!(cause.source().is_none(), "metadata is not a UTF-8 failure");
        }
    }
}

#[test]
fn valid_launch_refusals_keep_metadata_and_diagnostics_through_the_socket() {
    for typ in REJECTIONS {
        for kind in launch_kinds() {
            for diagnostic in ["", "loader — 音 🎹", "embedded\0nul"] {
                let failure = LaunchFailure {
                    kind,
                    diagnostic: diagnostic.into(),
                };
                let (id, message) = receive(typ, &failure.encode()).unwrap();
                assert_eq!(id, 73);
                let refusal = match message {
                    ControlMessage::PortsRejected(refusal)
                    | ControlMessage::StateRejected(refusal)
                    | ControlMessage::ParamsRejected(refusal)
                    | ControlMessage::GuiRejected(refusal) => refusal,
                    other => panic!("expected a refusal, got {other:?}"),
                };
                assert_eq!(refusal.to_string(), diagnostic);
                assert_eq!(refusal, Refusal::Launch(failure));
            }
        }
    }
}
