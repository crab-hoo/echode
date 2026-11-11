use super::*;
use crate::{ControlDecodeError, ControlField};
use std::io::Write;
use std::os::unix::net::UnixStream;

#[test]
fn parameter_operation_preserves_control_metadata_causes() {
    use ControlDecodeError::*;
    let mut cases = vec![
        (
            8u32,
            vec![0],
            WrongSize {
                field: ControlField::RestartLatency,
                expected: 4,
                actual: 1,
            },
        ),
        (
            12,
            vec![],
            WrongSize {
                field: ControlField::ChainStages,
                expected: 4,
                actual: 0,
            },
        ),
        (
            9,
            vec![],
            WrongSize {
                field: ControlField::RemapLayout,
                expected: 20,
                actual: 0,
            },
        ),
        (9, vec![0; 20], MissingMapping),
        (
            15,
            vec![],
            WrongSize {
                field: ControlField::PortTailLatency,
                expected: 8,
                actual: 0,
            },
        ),
        (
            15,
            vec![0; 8],
            WrongSize {
                field: ControlField::PortCount { input: true },
                expected: 4,
                actual: 0,
            },
        ),
        (
            15,
            vec![0; 12],
            WrongSize {
                field: ControlField::PortCount { input: false },
                expected: 4,
                actual: 0,
            },
        ),
        (15, vec![0; 17], TrailingPortBytes { actual: 1 }),
        (
            20,
            vec![0],
            WrongSize {
                field: ControlField::EmptyBody { message_type: 20 },
                expected: 0,
                actual: 1,
            },
        ),
        (
            32,
            vec![],
            WrongSize {
                field: ControlField::PluginCommand,
                expected: 1,
                actual: 0,
            },
        ),
        (32, vec![255], UnknownPluginCommand(255)),
        (u32::MAX, vec![], UnknownMessageType(u32::MAX)),
    ];
    for input in [true, false] {
        let mut body = if input {
            vec![]
        } else {
            0u32.to_le_bytes().to_vec()
        };
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&[0; 8]);
        cases.push((
            15,
            body,
            WrongSize {
                field: ControlField::PortChannels { input },
                expected: 4,
                actual: 0,
            },
        ));
    }
    for (typ, body, expected) in cases {
        let (parent, mut raw) = UnixStream::pair().unwrap();
        let control = PluginControl::new(Socket::from_owned(parent.into()).unwrap());
        let mut helper = Socket::from_owned(raw.try_clone().unwrap().into()).unwrap();
        let answer = std::thread::spawn(move || {
            let (id, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
            assert_ne!(id, 0);
            assert!(matches!(request, ControlMessage::ParamsRequest));
            let mut frame = (12 + body.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&typ.to_le_bytes());
            frame.extend_from_slice(&id.to_le_bytes());
            frame.extend_from_slice(&body);
            raw.write_all(&frame).unwrap();
        });
        let error = control.parameters().unwrap_err();
        answer.join().unwrap();
        let SessionError::Io(error) = error else {
            panic!("malformed metadata must remain an I/O cause: {error:?}");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), expected.to_string());
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<ControlDecodeError>(),
            Some(&expected),
        );
    }
}
