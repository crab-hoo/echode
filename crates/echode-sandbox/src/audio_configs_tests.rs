use super::*;
use crate::{ParamField, ParamPayload, PayloadDecodeError};

fn example() -> AudioPortConfig {
    AudioPortConfig {
        id: 206,
        name: "Mono 出力".into(),
        input_ports: 1,
        output_ports: 1,
        main_input_channels: 2,
        main_output_channels: 1,
        ports: Some(PortLayout::new(vec![2], vec![1])),
    }
}

#[test]
fn audio_configs_round_trip_and_reject_every_truncation_and_trailing_byte() {
    let configs = AudioPortConfigs {
        current_id: Some(303),
        configs: vec![
            example(),
            AudioPortConfig {
                id: 303,
                name: String::new(),
                input_ports: 2,
                main_input_channels: 1,
                main_output_channels: 2,
                ports: Some(PortLayout::new(vec![1, 1], vec![2])),
                ..example()
            },
            AudioPortConfig {
                id: 400,
                ports: None,
                ..example()
            },
        ],
    };
    let bytes = encode(&configs);
    assert_eq!(decode(&bytes).unwrap(), configs);
    for end in 0..bytes.len() {
        assert!(
            decode(&bytes[..end]).is_err(),
            "accepted truncated prefix {end}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    let error = decode(&trailing).unwrap_err();
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::TrailingBytes {
            payload: ParamPayload::AudioConfigs,
            actual: 1
        })
    );
    let empty = AudioPortConfigs {
        current_id: None,
        configs: vec![],
    };
    assert_eq!(decode(&encode(&empty)).unwrap(), empty);
}

#[test]
fn an_untrusted_count_is_bounded_by_payload_bytes_and_keeps_field_provenance() {
    let mut body = vec![0];
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    let error = decode(&body).unwrap_err();
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::Truncated {
            payload: ParamPayload::AudioConfigs,
            field: ParamField::Id,
            entry: Some(0),
            expected: 4,
            actual: 0
        })
    );
    let mut missing_current = vec![2];
    missing_current.extend_from_slice(&0u32.to_le_bytes());
    let error = decode(&missing_current).unwrap_err();
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::InvalidConfigFlag {
            payload: ParamPayload::AudioConfigs,
            field: ParamField::CurrentPresent,
            value: 2,
        })
    );
    // Reject bad PortsPresent flag and untrusted nested channel counts without OOM.
    let single = AudioPortConfigs {
        current_id: None,
        configs: vec![AudioPortConfig {
            ports: None,
            ..example()
        }],
    };
    let mut bad_ports_flag = encode(&single);
    bad_ports_flag[25] = 3;
    assert_eq!(
        decode(&bad_ports_flag)
            .unwrap_err()
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::InvalidConfigFlag {
            payload: ParamPayload::AudioConfigs,
            field: ParamField::PortsPresent,
            value: 3,
        })
    );
    let with_ports = AudioPortConfigs {
        current_id: None,
        configs: vec![example()],
    };
    let mut huge_inputs = encode(&with_ports);
    huge_inputs[26..30].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&huge_inputs).is_err());
    // Truncated name length / invalid UTF-8 keeps field provenance.
    let mut bad_name = encode(&single);
    let name_len_offset = 26;
    bad_name[name_len_offset..name_len_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bad_name).is_err());
    bad_name[name_len_offset..name_len_offset + 4].copy_from_slice(&1u32.to_le_bytes());
    bad_name[name_len_offset + 4] = 0xff;
    assert!(decode(&bad_name).is_err());
}

#[test]
fn audio_port_requests_and_applicable_round_trip_and_reject_invalid_frames() {
    let requests = vec![
        AudioPortRequest {
            input: true,
            index: 0,
            channels: 1,
            details: None,
        },
        AudioPortRequest {
            input: false,
            index: 2,
            channels: 6,
            details: Some(AudioPortDetails::Surround(vec![0, 1, 2, 3, 4, 5])),
        },
        AudioPortRequest {
            input: false,
            index: u32::MAX,
            channels: u32::MAX,
            details: Some(AudioPortDetails::Ambisonic {
                ordering: u32::MAX,
                normalization: u32::MAX,
            }),
        },
    ];
    let bytes = encode_requests(&requests);
    assert_eq!(decode_requests(&bytes).unwrap(), requests);
    for end in 0..bytes.len() {
        assert!(decode_requests(&bytes[..end]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode_requests(&trailing).is_err());
    let mut bad_side = bytes.clone();
    bad_side[4] = 2;
    assert_eq!(
        decode_requests(&bad_side)
            .unwrap_err()
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::InvalidConfigFlag {
            payload: ParamPayload::AudioPortRequests,
            field: ParamField::PortSide,
            value: 2,
        })
    );
    let mut bad_details = bytes.clone();
    bad_details[13] = 3;
    assert_eq!(
        decode_requests(&bad_details)
            .unwrap_err()
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::UnknownPortDetails(3))
    );
    let mut huge_map = bytes.clone();
    // First request is 10 bytes; second has its map count after 9 + 1 bytes.
    huge_map[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_requests(&huge_map).is_err());
    assert!(decode_requests(&u32::MAX.to_le_bytes()).is_err());
    for value in [false, true] {
        let encoded = encode_applicable(value);
        assert_eq!(decode_applicable(&encoded).unwrap(), value);
    }
    assert!(decode_applicable(&[]).is_err());
    assert!(decode_applicable(&[0, 1]).is_err());
    assert_eq!(
        decode_applicable(&[2])
            .unwrap_err()
            .get_ref()
            .unwrap()
            .downcast_ref::<PayloadDecodeError>(),
        Some(&PayloadDecodeError::InvalidConfigFlag {
            payload: ParamPayload::AudioPortsApplicable,
            field: ParamField::Applicable,
            value: 2,
        })
    );
}
