use super::*;
use crate::ControlTextError;
use std::str::Utf8Error;

fn text_messages(text: &str) -> Vec<ControlMessage> {
    let info = ParamInfo {
        id: 7,
        flags: 0,
        name: String::new(),
        module: String::new(),
        min: 0.0,
        max: 1.0,
        default: 0.5,
        value: 0.25,
        text: String::new(),
    };
    let value = ParamValue {
        id: 7,
        value: 0.25,
        text: text.into(),
    };
    vec![
        ControlMessage::GuiShow(text.into()),
        ControlMessage::ParseParam {
            id: 7,
            text: text.into(),
        },
        ControlMessage::Params(vec![ParamInfo {
            name: text.into(),
            ..info.clone()
        }]),
        ControlMessage::Params(vec![ParamInfo {
            module: text.into(),
            ..info.clone()
        }]),
        ControlMessage::Params(vec![ParamInfo {
            text: text.into(),
            ..info
        }]),
        ControlMessage::ParamsApplied(vec![value.clone()]),
        ControlMessage::ParamRelay(ParamRelay {
            values: vec![value],
            ..Default::default()
        }),
    ]
}

fn receive(typ: u32, body: &[u8]) -> io::Result<(u64, ControlMessage)> {
    let (sender, mut receiver) = Socket::pair()?;
    send_bytes(sender.as_raw_fd(), &frame_tagged(73, typ, body)?, None)?;
    receiver.recv_tagged(Duration::from_secs(2))
}

#[test]
fn invalid_control_text_retains_utf8_cause() {
    for message in text_messages("ab__") {
        let (typ, body) = encode(&message).unwrap();
        let offsets: Vec<_> = body
            .windows(4)
            .enumerate()
            .filter_map(|(at, bytes)| (bytes == b"ab__").then_some(at))
            .collect();
        assert_eq!(offsets.len(), 1, "the fixture has one text field");
        for bytes in [b"ab\xff!", b"ab\xe3\x81"] {
            let mut invalid = body.clone();
            invalid[offsets[0]..offsets[0] + 4].copy_from_slice(bytes);
            let error = receive(typ, &invalid).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            let expected = std::str::from_utf8(bytes).unwrap_err();
            let cause = error
                .get_ref()
                .and_then(|error| error.source())
                .and_then(|error| error.downcast_ref::<Utf8Error>())
                .expect("invalid control text must retain the UTF-8 decoding cause");
            assert_eq!(cause, &expected, "{message:?}");
            assert_eq!(cause.valid_up_to(), 2);
            assert_eq!(
                error.get_ref().unwrap().downcast_ref::<ControlTextError>(),
                Some(&if matches!(message, ControlMessage::GuiShow(_)) {
                    ControlTextError::WindowTitle(expected)
                } else {
                    ControlTextError::Parameter(expected)
                })
            );
            assert_eq!(
                error.to_string(),
                if matches!(message, ControlMessage::GuiShow(_)) {
                    "window title is not text"
                } else {
                    "parameter text is not UTF-8"
                }
            );
        }
    }
}

#[test]
fn valid_control_text_round_trips_without_normalization() {
    for text in ["", "Filter — 音 🎹", "embedded\0nul"] {
        for message in text_messages(text) {
            let (typ, body) = encode(&message).unwrap();
            let (id, decoded) = receive(typ, &body).unwrap();
            assert_eq!(id, 73);
            assert_eq!(format!("{decoded:?}"), format!("{message:?}"));
        }
    }
}

#[test]
fn malformed_parameter_structure_is_not_a_utf8_error() {
    for (typ, body) in [
        (PARSE_PARAM, vec![0, 0]),
        (PARAMS, vec![1, 0, 0, 0]),
        (PARAMS_APPLIED, vec![1, 0, 0, 0]),
        (PARAM_RELAY, vec![0]),
    ] {
        let error = receive(typ, &body).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "parameter message is truncated");
        assert!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<ControlTextError>())
                .is_none()
        );
    }
}
