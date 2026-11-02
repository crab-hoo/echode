//! Legacy journal vocabulary and value bytes, independent of the registries.
use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    AudioRef, Change, ChangeSet, ClipKind, Id, Meter, ObjectKey, Property, Tempo, Tick, Value,
};

#[test]
fn every_change_value_keeps_its_existing_journal_spelling() {
    let id = Id::from_raw(1);
    let uuid = "00000000-0000-0000-0000-000000000001";
    let cases = vec![
        (
            Value::Meter(Meter::try_new(7, 8).unwrap()),
            r#"{"type":"meter","numerator":7,"denominator":8}"#.into(),
        ),
        (
            Value::Tempo(Tempo::rational(241, 2).unwrap()),
            r#"{"type":"tempo","numerator":241,"denominator":2}"#.into(),
        ),
        (
            Value::Tick(Tick::new(i64::MIN)),
            format!(r#"{{"type":"tick","tick":{}}}"#, i64::MIN),
        ),
        (
            Value::Text("日本語\n\"".into()),
            "{\"type\":\"text\",\"text\":\"日本語\\n\\\"\"}".into(),
        ),
        (Value::Id(id), format!(r#"{{"type":"id","id":"{uuid}"}}"#)),
        (
            Value::Ids(vec![id, id]),
            format!(r#"{{"type":"ids","ids":["{uuid}","{uuid}"]}}"#),
        ),
        (
            Value::Parent(Some(id)),
            format!(r#"{{"type":"parent","id":"{uuid}"}}"#),
        ),
        (Value::Parent(None), r#"{"type":"parent","id":null}"#.into()),
        (
            Value::Audio(None),
            r#"{"type":"audio","hash":null,"sample_rate":0,"channels":0,"frames":0}"#.into(),
        ),
        (
            Value::Audio(Some(
                AudioRef::try_new("ab".repeat(32), 48_000, 2, u64::MAX).unwrap(),
            )),
            format!(
                r#"{{"type":"audio","hash":"{}","sample_rate":48000,"channels":2,"frames":{}}}"#,
                "ab".repeat(32),
                u64::MAX
            ),
        ),
        (
            Value::Kind(ClipKind::Auto),
            r#"{"type":"kind","kind":"auto"}"#.into(),
        ),
        (
            Value::Kind(ClipKind::Fixed),
            r#"{"type":"kind","kind":"fixed"}"#.into(),
        ),
        (Value::Byte(255), r#"{"type":"byte","value":255}"#.into()),
        (Value::U16(65535), r#"{"type":"u16","value":65535}"#.into()),
        (
            Value::Gain(0x8000_0000),
            r#"{"type":"gain","bits":2147483648}"#.into(),
        ),
        (
            Value::Bits(0x7fc0_0001),
            r#"{"type":"bits","bits":2143289345}"#.into(),
        ),
        (
            Value::Flicks(i64::MAX),
            format!(r#"{{"type":"flicks","value":{}}}"#, i64::MAX),
        ),
        (
            Value::ParamIds(vec![0, u32::MAX]),
            r#"{"type":"param_ids","ids":[0,4294967295]}"#.into(),
        ),
        (
            Value::OrderStep {
                index: u32::MAX,
                id,
                present: false,
            },
            format!(r#"{{"type":"order_step","index":4294967295,"id":"{uuid}","present":false}}"#),
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(serde_json::to_string(&value).unwrap(), expected);
        assert_eq!(serde_json::from_str::<Value>(&expected).unwrap(), value);
        let mut changes = ChangeSet::new();
        changes.push(Change::Update {
            id: ObjectKey::Project,
            property: Property::Value,
            before: value.clone(),
            after: value,
        });
        let line = JournalLine {
            seq: 1,
            op: JournalOp::Edit,
            actions: vec![],
            changes,
        };
        let encoded = encode_line(&line);
        assert!(encoded.contains(&format!("\"before\":{expected},\"after\":{expected}")));
        assert_eq!(decode_line(&encoded).unwrap(), line);
    }
    assert_eq!(
        serde_json::from_str::<Value>(r#"{"type":"audio","hash":null}"#).unwrap(),
        Value::Audio(None)
    );
}

#[test]
fn malformed_change_values_still_fail_at_the_journal_boundary() {
    for value in [
        r#"{"type":"meter","numerator":7,"denominator":0}"#,
        r#"{"type":"tempo","numerator":0,"denominator":1}"#,
        r#"{"type":"tick","tick":1.5}"#,
        r#"{"type":"text","text":null}"#,
        r#"{"type":"id","id":"invalid"}"#,
        r#"{"type":"ids","ids":["invalid"]}"#,
        r#"{"type":"parent","id":"invalid"}"#,
        r#"{"type":"audio","hash":"invalid","sample_rate":48000,"channels":2,"frames":1}"#,
        r#"{"type":"kind","kind":"other"}"#,
        r#"{"type":"byte","value":256}"#,
        r#"{"type":"u16","value":65536}"#,
        r#"{"type":"bits","bits":4294967296}"#,
        r#"{"type":"gain","bits":-1}"#,
        r#"{"type":"flicks","value":null}"#,
        r#"{"type":"param_ids","ids":[-1]}"#,
        r#"{"type":"order_step","index":0,"id":"invalid","present":true}"#,
        r#"{"type":"bits","bits":1,"unknown":true}"#,
        r#"{"type":"unknown","bits":1}"#,
        r#"{"type":"gain"}"#,
    ] {
        let line = format!(
            r#"{{"seq":1,"kind":"edit","changes":[{{"op":"update","id":"project","property":"value","before":{value},"after":{value}}}]}}"#
        );
        assert!(decode_line(&line).is_err(), "accepted {value}");
    }
}

#[test]
fn property_names_are_the_legacy_vocabulary_not_the_model_field_names() {
    let cases = [
        (Property::Meter, "meter"),
        (Property::TrackOrder, "track_order"),
        (Property::Name, "name"),
        (Property::Parent, "parent"),
        (Property::Order, "order"),
        (Property::ClipOrder, "clip_order"),
        (Property::Start, "start"),
        (Property::Length, "length"),
        (Property::Kind, "kind"),
        (Property::NoteOrder, "note_order"),
        (Property::Owner, "owner"),
        (Property::Pitch, "pitch"),
        (Property::Velocity, "velocity"),
        (Property::Release, "release"),
        (Property::Articulation, "articulation"),
        (Property::Tempo, "tempo"),
        (Property::Offset, "offset"),
        (Property::Gain, "gain"),
        (Property::FadeIn, "fade_in"),
        (Property::FadeOut, "fade_out"),
        (Property::Lane, "lane"),
        (Property::Content, "content"),
        (Property::Audio, "audio"),
        (Property::Takes, "takes"),
        (Property::Key, "key"),
        (Property::Minimum, "minimum"),
        (Property::Maximum, "maximum"),
        (Property::Default, "default"),
        (Property::Value, "value"),
        (Property::ParamOrder, "param_order"),
        (Property::DeviceKind, "device_kind"),
        (Property::DeviceOrder, "device_order"),
        (Property::Plugin, "plugin"),
        (Property::PluginState, "plugin_state"),
        (Property::Bypassed, "bypassed"),
        (Property::Offline, "offline"),
        (Property::PinnedParameters, "pinned_parameters"),
    ];
    for (property, text) in cases {
        assert_eq!(property.as_str(), text);
        assert_eq!(Property::parse(text), Some(property));
    }
    for text in ["min", "max", "state", "pinned", "Offline", "unknown"] {
        assert_eq!(Property::parse(text), None);
    }
}
