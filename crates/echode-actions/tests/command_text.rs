//! JSON text uses the registry's typed conversion without an intermediate Value.

use std::error::Error;

use echode_actions::{Command, ParameterError};
use echode_model::Id;
use serde_json::json;

#[test]
fn text_keeps_semantic_causes_and_registry_validation_order() {
    let invalid_id = "broken at line 7: tempo is outside the allowed range\n音";
    for value in [
        json!({"type": "track.duplicate", "source": invalid_id}),
        json!({"type": "device.copy", "source": "bad source", "track": "bad track", "before": "bad before"}),
        json!({"type": "project.set_meter", "numerator": 1, "denominator": 27}),
        json!({"type": "project.set_tempo", "at": 0, "numerator": 120, "denominator": 0}),
    ] {
        let text = serde_json::to_string_pretty(&value).unwrap();
        let error = text.parse::<Command>().unwrap_err();
        let expected = Command::from_value(value).unwrap_err();
        assert!(!matches!(&error, ParameterError::Json(_)));
        assert_eq!(format!("{error:?}"), format!("{expected:?}"));
        assert_eq!(error.to_string(), expected.to_string());
        assert!(error.source().is_none());
    }
}

#[test]
fn text_checks_shape_and_end_of_input_before_semantic_values() {
    for text in [
        "{\n",
        "null",
        r#"{"type":"track.add","name":"x","name":"y"}"#,
        r#"{"type":"track.add","name":"x"} {}"#,
        r#"{"type":"device.copy","source":"bad source","track":7}"#,
        r#"{"type":"track.duplicate","source":"bad source"} {}"#,
        r#"{"type":"meter does not divide into ticks"}"#,
    ] {
        let error = text.parse::<Command>().unwrap_err();
        assert!(
            matches!(&error, ParameterError::Json(_)),
            "{text}: {error:?}"
        );
        assert!(error.source().unwrap().is::<serde_json::Error>());
    }
}

#[test]
fn text_and_value_entries_keep_defaults_flattening_and_numeric_boundaries() {
    let id = Id::generate();
    for value in [
        json!({"type": "track.add", "name": "音"}),
        json!({"type": "project.set_meter", "numerator": 7, "denominator": 8}),
        json!({"type": "project.set_tempo", "at": i64::MIN, "numerator": 241, "denominator": 2}),
        json!({"type": "device.reorder", "device": id}),
        json!({"type": "device.reorder", "device": id, "before": null}),
        json!({"type": "device.reorder", "device": id, "before": id}),
        json!({"type": "note.insert", "track": id, "pitch": 127, "velocity": 0,
               "start": i64::MIN, "length": i64::MAX}),
        json!({"type": "audio.place", "track": id, "start": i64::MAX,
               "hash": "a".repeat(64), "sample_rate": u32::MAX, "channels": u16::MAX,
               "frames": u64::MAX}),
        json!({"type": "param.set", "parameter": id, "value": -0.0}),
    ] {
        let text = serde_json::to_string(&value).unwrap();
        let decoded = text.parse::<Command>().unwrap();
        assert_eq!(decoded, Command::from_value(value).unwrap());
        assert_eq!(decoded, serde_json::from_str::<Command>(&text).unwrap());
        assert_eq!(
            serde_json::to_string(&decoded)
                .unwrap()
                .parse::<Command>()
                .unwrap(),
            decoded
        );
    }
}
