use super::*;
use crate::{RefusalDecodeError, RefusalField};
use std::io::Write;
use std::os::unix::net::UnixStream;

#[test]
fn parameter_operation_preserves_malformed_refusal_structure_causes() {
    use RefusalDecodeError::*;
    for (payload, expected) in [
        (vec![], MissingKind),
        (vec![255], UnknownKind(255)),
        (vec![1, 0], TrailingBytes { kind: 1, actual: 1 }),
        (
            vec![3, 0],
            WrongSize {
                field: RefusalField::MainCall,
                expected: 8,
                actual: 1,
            },
        ),
        (
            vec![9],
            WrongSize {
                field: RefusalField::ParameterId,
                expected: 4,
                actual: 0,
            },
        ),
        (
            vec![10, 0],
            WrongSize {
                field: RefusalField::MainReplies,
                expected: 2,
                actual: 1,
            },
        ),
        (vec![10, 6, 0], UnknownExpectedReply(6)),
        (vec![10, 0, 6], UnknownActualReply(6)),
        (vec![7], MissingPluginKind),
        (vec![7, 255], UnknownPluginKind(255)),
        (
            vec![7, 18],
            WrongSize {
                field: RefusalField::PluginPayload { kind: 18 },
                expected: 5,
                actual: 0,
            },
        ),
        (
            vec![7, 19, 2, 0, 0, 0, 0],
            InvalidPortSide { kind: 19, side: 2 },
        ),
    ] {
        let (parent, mut raw) = UnixStream::pair().unwrap();
        let control = PluginControl::new(Socket::from_owned(parent.into()).unwrap());
        let mut helper = Socket::from_owned(raw.try_clone().unwrap().into()).unwrap();
        let answer = std::thread::spawn(move || {
            let (id, request) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
            assert_ne!(id, 0);
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
            panic!("malformed refusal must remain an I/O cause: {error:?}");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), expected.to_string());
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<RefusalDecodeError>(),
            Some(&expected)
        );
    }
}
