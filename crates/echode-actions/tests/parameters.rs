//! The typed and Serde command entries share shape and model validation.

use std::error::Error;

use echode_actions::{Command, ParameterError};
use echode_model::{Id, Meter, Tempo, Tick};
use serde_json::{Value, json};

#[test]
fn value_failures_keep_the_rejected_values_not_serde_messages() {
    let invalid_id = "broken at line 7: meter does not divide into ticks\n音";
    let error =
        Command::from_value(json!({"type": "track.duplicate", "source": invalid_id})).unwrap_err();
    assert!(matches!(
        &error,
        ParameterError::InvalidId { field: "source", value } if value == invalid_id
    ));
    assert_eq!(error.to_string(), format!("invalid id {invalid_id}"));
    assert!(error.source().is_none());

    let error = Command::from_value(json!({
        "type": "project.set_meter", "numerator": 1, "denominator": 27
    }))
    .unwrap_err();
    assert!(matches!(
        error,
        ParameterError::InvalidMeter {
            numerator: 1,
            denominator: 27
        }
    ));
    assert_eq!(error.to_string(), "meter does not divide into ticks");

    for (numerator, denominator) in [(0, 1), (120, 0), (u32::MAX, 1)] {
        let error = Command::from_value(json!({
            "type": "project.set_tempo", "at": 0,
            "numerator": numerator, "denominator": denominator
        }))
        .unwrap_err();
        assert!(matches!(
            error,
            ParameterError::InvalidTempo { numerator: n, denominator: d }
                if n == numerator && d == denominator
        ));
        assert_eq!(error.to_string(), "tempo is outside the allowed range");
    }
}

#[test]
fn shape_failures_retain_the_json_cause_without_classifying_its_text() {
    for value in [
        Value::Null,
        json!([]),
        json!({"type": "invalid id user text"}),
        json!({"type": "track.add", "name": 7}),
        json!({"type": "track.add", "name": "x", "tempo is outside the allowed range": 0}),
        json!({"type": "project.set_meter", "numerator": 1, "denominator": "7"}),
        json!({"type": "project.set_meter", "numerator": -1, "denominator": 4}),
        json!({"type": "project.set_tempo", "at": 0, "numerator": 120}),
        json!({"type": "track.duplicate", "source": 42}),
        json!({"type": "note.insert", "track": Id::generate(), "pitch": 256,
               "velocity": 100, "start": 0, "length": 1}),
    ] {
        let error = Command::from_value(value.clone()).unwrap_err();
        assert!(
            matches!(error, ParameterError::Json(_)),
            "{value}: {error:?}"
        );
        assert!(error.source().unwrap().is::<serde_json::Error>());
        assert_eq!(error.to_string(), "parameters do not match an action");
        assert!(serde_json::from_value::<Command>(value).is_err());
    }
}

#[test]
fn optional_ids_validate_present_text_and_keep_null_and_missing_defaults() {
    let device = Id::generate();
    for kind in [
        "device.reorder",
        "device.copy",
        "note.duplicate",
        "track.set_parent",
    ] {
        let mut value = json!({
            "device.reorder": {"type": kind, "device": device},
            "device.copy": {"type": kind, "source": device, "track": device},
            "note.duplicate": {"type": kind, "track": device, "clip": device,
                               "note": device, "pitch": 60, "start": 0},
            "track.set_parent": {"type": kind, "track": device},
        })[kind]
            .clone();
        let field = match kind {
            "note.duplicate" => "copy",
            "track.set_parent" => "parent",
            _ => "before",
        };
        let missing = Command::from_value(value.clone()).unwrap();
        value[field] = Value::Null;
        assert_eq!(Command::from_value(value.clone()).unwrap(), missing);
        value[field] = json!(device);
        assert_eq!(
            Command::from_value(value.clone()).unwrap(),
            serde_json::from_value::<Command>(value.clone()).unwrap()
        );
        value[field] = json!("broken");
        let error = Command::from_value(value).unwrap_err();
        assert!(matches!(
            error,
            ParameterError::InvalidId { field: found, value } if found == field && value == "broken"
        ));
    }
}

#[test]
fn flattened_values_and_integer_endpoints_still_round_trip() {
    for (value, expected) in [
        (
            json!({"type": "project.set_meter", "numerator": 7, "denominator": 8}),
            Command::SetMeter {
                meter: Meter::try_new(7, 8).unwrap(),
            },
        ),
        (
            json!({"type": "project.set_tempo", "at": i64::MIN,
                   "numerator": 241, "denominator": 2}),
            Command::SetTempo {
                at: Tick::new(i64::MIN),
                tempo: Tempo::rational(241, 2).unwrap(),
            },
        ),
    ] {
        assert_eq!(Command::from_value(value.clone()).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<Command>(value.clone()).unwrap(),
            expected
        );
        assert_eq!(serde_json::to_value(expected).unwrap(), value);
    }

    let track = Id::generate();
    for value in [
        json!({"type": "note.insert", "track": track, "pitch": 127, "velocity": 0,
               "start": i64::MIN, "length": i64::MAX}),
        json!({"type": "audio.place", "track": track, "start": i64::MAX,
               "hash": "a".repeat(64), "sample_rate": u32::MAX, "channels": u16::MAX,
               "frames": u64::MAX}),
        json!({"type": "param.set", "parameter": track, "value": -0.0}),
    ] {
        let typed = Command::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::from_value::<Command>(value.clone()).unwrap(),
            typed
        );
        assert_eq!(serde_json::to_value(typed).unwrap(), value);
    }
}

#[test]
fn shape_precedes_value_validation_and_values_follow_registry_order() {
    let error = Command::from_value(json!({
        "type": "device.copy", "source": "bad source", "track": "bad track", "before": "bad before"
    }))
    .unwrap_err();
    assert!(matches!(
        error,
        ParameterError::InvalidId {
            field: "source",
            ..
        }
    ));
    let error = Command::from_value(json!({
        "type": "device.copy", "source": "bad source", "track": 7
    }))
    .unwrap_err();
    assert!(matches!(error, ParameterError::Json(_)));
}
