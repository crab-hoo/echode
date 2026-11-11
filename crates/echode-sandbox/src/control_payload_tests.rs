use super::*;
use crate::gui::{ParamEvent, ParamEventKind};
use crate::params::{write_changes, write_infos, write_values};
use crate::{ParamField as F, ParamPayload, PayloadDecodeError, WindowFlag};

fn receive(typ: u32, body: &[u8]) -> io::Result<(u64, ControlMessage)> {
    let (sender, mut receiver) = Socket::pair()?;
    send_bytes(sender.as_raw_fd(), &frame_tagged(73, typ, body)?, None)?;
    receiver.recv_tagged(Duration::from_secs(2))
}

fn assert_cause(error: io::Error, expected: PayloadDecodeError, diagnostic: &str) {
    assert_eq!(error.kind(), ErrorKind::InvalidData, "{expected:?}");
    assert_eq!(error.to_string(), diagnostic, "{expected:?}");
    let cause = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<PayloadDecodeError>())
        .expect("malformed parameter payload must retain its typed structural cause");
    assert_eq!(cause, &expected);
    assert!(std::error::Error::source(cause).is_none());
}

/// One field of a valid payload: what the reader calls it, the list entry it
/// belongs to, and its bytes. Written apart from the product reader on purpose.
struct Part(F, Option<usize>, Vec<u8>);

fn count(len: usize) -> Vec<u8> {
    (len as u32).to_le_bytes().to_vec()
}

fn info_parts(infos: &[ParamInfo]) -> Vec<Part> {
    let mut parts = vec![Part(F::Count, None, count(infos.len()))];
    for (entry, info) in infos.iter().enumerate() {
        let entry = Some(entry);
        parts.push(Part(F::Id, entry, info.id.to_le_bytes().to_vec()));
        parts.push(Part(F::Flags, entry, info.flags.to_le_bytes().to_vec()));
        for (field, value) in [
            (F::Min, info.min),
            (F::Max, info.max),
            (F::Default, info.default),
            (F::Value, info.value),
        ] {
            parts.push(Part(field, entry, value.to_bits().to_le_bytes().to_vec()));
        }
        for (length, field, text) in [
            (F::NameLength, F::Name, &info.name),
            (F::ModuleLength, F::Module, &info.module),
            (F::TextLength, F::Text, &info.text),
        ] {
            parts.push(Part(length, entry, count(text.len())));
            parts.push(Part(field, entry, text.as_bytes().to_vec()));
        }
    }
    parts
}

fn value_parts(values: &[ParamValue]) -> Vec<Part> {
    let mut parts = vec![Part(F::Count, None, count(values.len()))];
    for (entry, value) in values.iter().enumerate() {
        let entry = Some(entry);
        parts.push(Part(F::Id, entry, value.id.to_le_bytes().to_vec()));
        let bits = value.value.to_bits().to_le_bytes().to_vec();
        parts.push(Part(F::Value, entry, bits));
        parts.push(Part(F::TextLength, entry, count(value.text.len())));
        parts.push(Part(F::Text, entry, value.text.as_bytes().to_vec()));
    }
    parts
}

fn change_parts(changes: &[ParamChange]) -> Vec<Part> {
    let mut parts = vec![Part(F::Count, None, count(changes.len()))];
    for (entry, change) in changes.iter().enumerate() {
        let entry = Some(entry);
        parts.push(Part(F::Id, entry, change.id.to_le_bytes().to_vec()));
        let bits = change.value.to_bits().to_le_bytes().to_vec();
        parts.push(Part(F::Value, entry, bits));
    }
    parts
}

fn relay_parts(relay: &ParamRelay) -> Vec<Part> {
    let flags = u8::from(relay.rescan) | u8::from(relay.lost) << 1;
    let mut parts = vec![
        Part(F::RelayFlags, None, vec![flags]),
        Part(F::EventCount, None, count(relay.events.len())),
    ];
    for (entry, event) in relay.events.iter().enumerate() {
        let entry = Some(entry);
        parts.push(Part(F::EventId, entry, event.id.to_le_bytes().to_vec()));
        parts.push(Part(F::EventKind, entry, vec![event.kind as u8]));
    }
    parts.extend(value_parts(&relay.values));
    parts
}

fn bytes(parts: &[Part]) -> Vec<u8> {
    parts.iter().flat_map(|part| part.2.clone()).collect()
}

/// The field that runs out when only `cut` bytes of the payload arrive.
fn truncation(payload: ParamPayload, parts: &[Part], cut: usize) -> PayloadDecodeError {
    let mut start = 0;
    for Part(field, entry, bytes) in parts {
        let end = start + bytes.len();
        // An empty text is complete at its own start, so the next field fails.
        if cut < end {
            return PayloadDecodeError::Truncated {
                payload,
                field: *field,
                entry: *entry,
                expected: bytes.len(),
                actual: cut - start,
            };
        }
        start = end;
    }
    panic!("cut {cut} is not inside the payload");
}

fn assert_every_cut(message: &ControlMessage, payload: ParamPayload, parts: &[Part]) {
    let (typ, body) = encode(message).unwrap();
    assert_eq!(
        bytes(parts),
        body,
        "the model must match the product writer"
    );
    for cut in 0..body.len() {
        assert_cause(
            receive(typ, &body[..cut]).unwrap_err(),
            truncation(payload, parts, cut),
            "parameter message is truncated",
        );
    }
}

fn infos() -> Vec<ParamInfo> {
    let gain = ParamInfo {
        id: 7,
        flags: crate::PARAM_STEPPED,
        name: "Gain".into(),
        module: String::new(),
        min: 0.0,
        max: 2.0,
        default: 1.0,
        value: 0.5,
        text: "0.50×".into(),
    };
    let unnamed = ParamInfo {
        id: u32::MAX,
        name: String::new(),
        module: "Main/Out".into(),
        text: String::new(),
        ..gain.clone()
    };
    vec![gain, unnamed]
}

fn values() -> Vec<ParamValue> {
    vec![
        ParamValue {
            id: 4,
            value: -1.5,
            text: "−1.5 dB".into(),
        },
        ParamValue {
            id: 5,
            value: 0.0,
            text: String::new(),
        },
    ]
}

fn changes() -> Vec<ParamChange> {
    vec![
        ParamChange { id: 1, value: 0.25 },
        ParamChange { id: 9, value: -2.0 },
    ]
}

fn relay() -> ParamRelay {
    let event = |id, kind| ParamEvent { id, kind };
    ParamRelay {
        events: vec![
            event(4, ParamEventKind::Begin),
            event(4, ParamEventKind::Value),
            event(5, ParamEventKind::End),
        ],
        values: values(),
        rescan: true,
        lost: false,
    }
}

#[test]
fn truncated_payloads_retain_the_payload_field_entry_and_lengths() {
    use ParamPayload::*;
    assert_every_cut(
        &ControlMessage::Params(infos()),
        Parameters,
        &info_parts(&infos()),
    );
    assert_every_cut(
        &ControlMessage::SetParams(changes()),
        Changes,
        &change_parts(&changes()),
    );
    assert_every_cut(
        &ControlMessage::ParamsApplied(values()),
        AppliedValues,
        &value_parts(&values()),
    );
    assert_every_cut(
        &ControlMessage::ParamRelay(relay()),
        Relay,
        &relay_parts(&relay()),
    );
    assert_every_cut(
        &ControlMessage::ParamParsed(ParamText::Value(-0.5)),
        ParsedText,
        &[
            Part(F::ParsedKind, None, vec![0]),
            Part(F::Value, None, (-0.5f64).to_bits().to_le_bytes().to_vec()),
        ],
    );
    for actual in 0..4 {
        assert_cause(
            receive(PARSE_PARAM, &vec![0; actual]).unwrap_err(),
            PayloadDecodeError::Truncated {
                payload: TextRequest,
                field: F::Id,
                entry: None,
                expected: 4,
                actual,
            },
            "parameter message is truncated",
        );
    }
}

#[test]
fn declared_counts_and_lengths_fail_at_the_first_missing_field() {
    use ParamPayload::*;
    let huge = u32::MAX.to_le_bytes();
    let missing = |payload, field, entry| PayloadDecodeError::Truncated {
        payload,
        field,
        entry,
        expected: 4,
        actual: 0,
    };
    // A huge declared count must fail before allocation from that count.
    for (typ, prefix, payload, field) in [
        (PARAMS, vec![], Parameters, F::Id),
        (SET_PARAMS, vec![], Changes, F::Id),
        (PARAMS_APPLIED, vec![], AppliedValues, F::Id),
        (PARAM_RELAY, vec![0], Relay, F::EventId),
        (PARAM_RELAY, vec![0, 0, 0, 0, 0], Relay, F::Id),
    ] {
        let mut body = prefix;
        body.extend_from_slice(&huge);
        assert_cause(
            receive(typ, &body).unwrap_err(),
            missing(payload, field, Some(0)),
            "parameter message is truncated",
        );
    }
    // A text length beyond the payload reports the declared and present bytes.
    let parts = info_parts(&infos());
    let name_length = parts
        .iter()
        .position(|part| part.0 == F::NameLength)
        .unwrap();
    let mut body = bytes(&parts[..name_length]);
    body.extend_from_slice(&huge);
    body.extend_from_slice(b"Ga");
    assert_cause(
        receive(PARAMS, &body).unwrap_err(),
        PayloadDecodeError::Truncated {
            payload: Parameters,
            field: F::Name,
            entry: Some(0),
            expected: u32::MAX as usize,
            actual: 2,
        },
        "parameter message is truncated",
    );
}

#[test]
fn trailing_bytes_retain_the_payload_and_their_count() {
    use ParamPayload::*;
    let parsed = |parsed| encode(&ControlMessage::ParamParsed(parsed)).unwrap().1;
    for (typ, payload, body) in [
        (PARAMS, Parameters, write_infos(&infos())),
        (PARAMS, Parameters, write_infos(&[])),
        (SET_PARAMS, Changes, write_changes(&changes())),
        (PARAMS_APPLIED, AppliedValues, write_values(&values())),
        (PARAM_PARSED, ParsedText, parsed(ParamText::Value(3.0))),
        (PARAM_PARSED, ParsedText, parsed(ParamText::Refused)),
        (PARAM_PARSED, ParsedText, parsed(ParamText::Unsupported)),
        (PARAM_RELAY, Relay, bytes(&relay_parts(&relay()))),
    ] {
        for actual in 1..=3 {
            let mut longer = body.clone();
            longer.extend(std::iter::repeat_n(0, actual));
            assert_cause(
                receive(typ, &longer).unwrap_err(),
                PayloadDecodeError::TrailingBytes { payload, actual },
                "parameter message has trailing bytes",
            );
        }
    }
}

#[test]
fn non_finite_values_retain_their_entry_id_and_bits() {
    let non_finite = [
        f64::NAN.to_bits(),
        f64::INFINITY.to_bits(),
        f64::NEG_INFINITY.to_bits(),
        // A signalling NaN with a payload must arrive unchanged in the cause.
        0x7ff0_0000_0000_0001,
    ];
    for bits in non_finite {
        for entry in 0..2 {
            let mut list = changes();
            list[entry].value = f64::from_bits(bits);
            let cause = PayloadDecodeError::NonFiniteChange {
                entry,
                id: list[entry].id,
                bits,
            };
            let body = write_changes(&list);
            // The value is rejected where it is read: before trailing bytes
            // and, for the first entry, before the last entry's truncation.
            let mut bodies = vec![body.clone(), [body.clone(), vec![0]].concat()];
            if entry == 0 {
                bodies.push(body[..body.len() - 1].to_vec());
            }
            for body in bodies {
                assert_cause(
                    receive(SET_PARAMS, &body).unwrap_err(),
                    cause,
                    "parameter value is not finite",
                );
            }
        }
        let mut body = vec![0];
        body.extend_from_slice(&bits.to_le_bytes());
        for body in [body.clone(), [body, vec![0]].concat()] {
            assert_cause(
                receive(PARAM_PARSED, &body).unwrap_err(),
                PayloadDecodeError::NonFiniteParsedValue { bits },
                "parameter value is not finite",
            );
        }
    }
}

#[test]
fn unknown_codes_retain_the_received_value() {
    for kind in 3..=u8::MAX {
        for body in [vec![kind], vec![kind, 0, 0]] {
            assert_cause(
                receive(PARAM_PARSED, &body).unwrap_err(),
                PayloadDecodeError::UnknownParsedKind(kind),
                "unknown parameter text reply",
            );
        }
    }
    let valid = bytes(&relay_parts(&relay()));
    for flags in 4..=u8::MAX {
        let mut rest = valid.clone();
        rest[0] = flags;
        // Unknown flags are rejected before the rest of the relay is read.
        for body in [vec![flags], rest] {
            assert_cause(
                receive(PARAM_RELAY, &body).unwrap_err(),
                PayloadDecodeError::UnknownRelayFlags(flags),
                "relay flags are unknown",
            );
        }
    }
    for (entry, event) in relay().events.iter().enumerate() {
        for kind in 3..=u8::MAX {
            let mut body = valid.clone();
            // One flag byte, the count, then five bytes for each event.
            body[5 + entry * 5 + 4] = kind;
            assert_cause(
                receive(PARAM_RELAY, &body).unwrap_err(),
                PayloadDecodeError::UnknownEventKind {
                    entry,
                    id: event.id,
                    kind,
                },
                "relay event kind is unknown",
            );
        }
    }
}

#[test]
fn window_reports_retain_their_size_and_the_first_invalid_flag() {
    // These byte lengths bound only the malformed fixture, not product metadata.
    for actual in (0..=8).filter(|actual| *actual != 3) {
        assert_cause(
            receive(GUI_SHOWN, &vec![2; actual]).unwrap_err(),
            PayloadDecodeError::WindowReportSize {
                expected: 3,
                actual,
            },
            "window report is the wrong size",
        );
    }
    let flags = [WindowFlag::Created, WindowFlag::Visible, WindowFlag::Key];
    for (at, flag) in flags.into_iter().enumerate() {
        for value in 2..=u8::MAX {
            // Later flags are invalid too; the first one in wire order is reported.
            let mut body = [value; 3];
            body[..at].fill(1);
            assert_cause(
                receive(GUI_SHOWN, &body).unwrap_err(),
                PayloadDecodeError::InvalidWindowFlag { flag, value },
                "window report flag is not 0 or 1",
            );
        }
    }
}

#[test]
fn valid_payloads_keep_their_values_and_existing_acceptance() {
    // The reader does not judge described or read-back values: the helper
    // skips unusable ranges, and the host checks `ParamInfo::usable`.
    let strange = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0];
    let mut described = infos();
    let mut read_back = values();
    for (at, value) in strange.into_iter().enumerate() {
        described.push(ParamInfo {
            id: at as u32,
            min: value,
            max: value,
            default: value,
            value,
            ..infos()[0].clone()
        });
        read_back.push(ParamValue {
            id: at as u32,
            value,
            text: "?".into(),
        });
    }
    let mut messages = vec![
        ControlMessage::Params(vec![]),
        ControlMessage::Params(described),
        ControlMessage::SetParams(vec![]),
        ControlMessage::SetParams(changes()),
        ControlMessage::ParamsApplied(vec![]),
        ControlMessage::ParamsApplied(read_back.clone()),
        ControlMessage::ParseParam {
            id: 9,
            text: String::new(),
        },
        ControlMessage::ParseParam {
            id: u32::MAX,
            text: "−3 dB".into(),
        },
        ControlMessage::ParamParsed(ParamText::Value(-0.0)),
        ControlMessage::ParamParsed(ParamText::Refused),
        ControlMessage::ParamParsed(ParamText::Unsupported),
        ControlMessage::ParamRelay(ParamRelay::default()),
        ControlMessage::ParamRelay(relay()),
    ];
    for flags in 0..4u8 {
        messages.push(ControlMessage::ParamRelay(ParamRelay {
            values: read_back.clone(),
            rescan: flags & 1 != 0,
            lost: flags & 2 != 0,
            ..relay()
        }));
    }
    for flags in 0..8u8 {
        messages.push(ControlMessage::GuiShown(GuiReport {
            created: flags & 1 != 0,
            visible: flags & 2 != 0,
            key: flags & 4 != 0,
        }));
    }
    for message in messages {
        let (typ, body) = encode(&message).unwrap();
        let (id, actual) = receive(typ, &body).unwrap();
        assert_eq!(id, 73);
        assert_eq!(format!("{actual:?}"), format!("{message:?}"));
        // Bit-exact, including the sign of zero and the NaN the writer sent.
        assert_eq!(encode(&actual).unwrap(), (typ, body));
    }
}
