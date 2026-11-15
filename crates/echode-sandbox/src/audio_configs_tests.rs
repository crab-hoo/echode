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
    }
}

#[test]
fn audio_configs_round_trip_and_reject_every_truncation_and_trailing_byte() {
    let configs = vec![
        example(),
        AudioPortConfig {
            name: String::new(),
            ..example()
        },
    ];
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
    assert_eq!(decode(&encode(&[])).unwrap(), []);
}

#[test]
fn an_untrusted_count_is_bounded_by_payload_bytes_and_keeps_field_provenance() {
    let error = decode(&u32::MAX.to_le_bytes()).unwrap_err();
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
    let mut bytes = encode(&[example()]);
    bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&bytes).is_err());
    bytes[24..28].copy_from_slice(&1u32.to_le_bytes());
    bytes[28] = 0xff;
    assert!(decode(&bytes).is_err());
}
