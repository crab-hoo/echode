use super::*;
use crate::{ParamChange, ParamField, ParamPayload, PayloadDecodeError, WindowFlag};
use std::io::Write;
use std::os::unix::net::UnixStream;

/// Runs one public operation against a helper that answers its request with
/// a raw frame, bypassing the typed sender.
fn answered<T: std::fmt::Debug>(
    operation: impl FnOnce(&PluginControl) -> Result<T, SessionError>,
    typ: u32,
    body: Vec<u8>,
) -> std::io::Error {
    let (parent, mut raw) = UnixStream::pair().unwrap();
    let control = PluginControl::new(Socket::from_owned(parent.into()).unwrap());
    let mut helper = Socket::from_owned(raw.try_clone().unwrap().into()).unwrap();
    let answer = std::thread::spawn(move || {
        let (id, _) = helper.recv_tagged(Duration::from_secs(2)).unwrap();
        assert_ne!(id, 0, "operations must use correlated requests");
        let mut frame = (12 + body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&typ.to_le_bytes());
        frame.extend_from_slice(&id.to_le_bytes());
        frame.extend_from_slice(&body);
        raw.write_all(&frame).unwrap();
    });
    let error = operation(&control).unwrap_err();
    answer.join().unwrap();
    let SessionError::Io(error) = error else {
        panic!("a malformed payload must remain an I/O cause: {error:?}");
    };
    error
}

fn assert_cause(error: std::io::Error, expected: PayloadDecodeError) {
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), expected.to_string());
    assert_eq!(
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<PayloadDecodeError>()),
        Some(&expected),
    );
}

#[test]
fn operations_preserve_parameter_payload_causes() {
    use PayloadDecodeError::*;
    // Wire types: Params 21, ParamsApplied 23, ParamParsed 34.
    let truncated = |payload, field, entry, expected, actual| Truncated {
        payload,
        field,
        entry,
        expected,
        actual,
    };
    for (body, expected) in [
        (
            vec![],
            truncated(ParamPayload::Parameters, ParamField::Count, None, 4, 0),
        ),
        (
            1u32.to_le_bytes().to_vec(),
            truncated(ParamPayload::Parameters, ParamField::Id, Some(0), 4, 0),
        ),
        (
            vec![0; 5],
            TrailingBytes {
                payload: ParamPayload::Parameters,
                actual: 1,
            },
        ),
    ] {
        assert_cause(answered(PluginControl::parameters, 21, body), expected);
    }
    let change = [ParamChange { id: 3, value: 0.5 }];
    let mut cut = 1u32.to_le_bytes().to_vec();
    cut.extend_from_slice(&3u32.to_le_bytes());
    cut.extend_from_slice(&[0; 3]);
    for (body, expected) in [
        (
            cut,
            truncated(
                ParamPayload::AppliedValues,
                ParamField::Value,
                Some(0),
                8,
                3,
            ),
        ),
        (
            vec![0; 6],
            TrailingBytes {
                payload: ParamPayload::AppliedValues,
                actual: 2,
            },
        ),
    ] {
        assert_cause(
            answered(|control| control.set_parameters(&change), 23, body),
            expected,
        );
    }
    let mut not_finite = vec![0];
    not_finite.extend_from_slice(&f64::NAN.to_bits().to_le_bytes());
    for (body, expected) in [
        (
            vec![],
            truncated(ParamPayload::ParsedText, ParamField::ParsedKind, None, 1, 0),
        ),
        (
            not_finite,
            NonFiniteParsedValue {
                bits: f64::NAN.to_bits(),
            },
        ),
        (vec![9], UnknownParsedKind(9)),
        (
            vec![1, 0],
            TrailingBytes {
                payload: ParamPayload::ParsedText,
                actual: 1,
            },
        ),
    ] {
        assert_cause(
            answered(|control| control.parse_parameter(3, "1"), 34, body),
            expected,
        );
    }
}

#[test]
fn operations_preserve_window_report_and_relay_causes() {
    use PayloadDecodeError::*;
    // Wire types: GuiShown 26, ParamRelay 31.
    for (body, expected) in [
        (
            vec![1, 1],
            WindowReportSize {
                expected: 3,
                actual: 2,
            },
        ),
        (
            vec![1, 0, 7],
            InvalidWindowFlag {
                flag: WindowFlag::Key,
                value: 7,
            },
        ),
    ] {
        assert_cause(
            answered(|control| control.show_gui("Test"), 26, body),
            expected,
        );
    }
    // A relay is a notice, but a damaged one still fails the operation that
    // reads it from the socket rather than being dropped.
    let mut unknown_kind = vec![0];
    unknown_kind.extend_from_slice(&1u32.to_le_bytes());
    unknown_kind.extend_from_slice(&8u32.to_le_bytes());
    unknown_kind.push(3);
    for (body, expected) in [
        (vec![4], UnknownRelayFlags(4)),
        (
            unknown_kind,
            UnknownEventKind {
                entry: 0,
                id: 8,
                kind: 3,
            },
        ),
        (
            vec![0, 0, 0, 0, 0],
            Truncated {
                payload: ParamPayload::Relay,
                field: ParamField::Count,
                entry: None,
                expected: 4,
                actual: 0,
            },
        ),
    ] {
        assert_cause(answered(PluginControl::parameters, 31, body), expected);
    }
}
