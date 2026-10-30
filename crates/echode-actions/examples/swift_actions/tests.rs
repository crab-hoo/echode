use super::*;
use echode_actions::Command;
use serde_json::json;

#[test]
fn committed_swift_matches_every_catalog_schema() {
    let committed =
        include_str!("../../../../swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift");
    assert_eq!(
        generate(catalog()).unwrap(),
        committed,
        "run make swift-actions"
    );
    assert_eq!(committed.matches("    case `").count(), catalog().len());
}

#[test]
fn swift_wire_fixtures_cover_the_catalog_and_roundtrip_through_rust_commands() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../swift/EchodeBridge/Tests/EchodeBridgeTests/Fixtures/edit-actions.json"
    ))
    .unwrap();
    let mut covered = BTreeSet::new();
    for fixture in fixtures {
        let command: Command = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(command).unwrap(), fixture);
        assert!(covered.insert(fixture["type"].as_str().unwrap().to_string()));
    }
    assert_eq!(
        covered,
        catalog()
            .iter()
            .map(|action| action.id.to_string())
            .collect()
    );
}

#[test]
fn unsupported_schema_shapes_do_not_silently_weaken_types() {
    for schema in [
        json!({"type": "object", "properties": {}}),
        json!({"type": "array", "items": {"type": "string"}}),
        json!({"type": "integer", "format": "uint128"}),
        json!({"type": "string", "enum": ["only"]}),
        json!({"$ref": "#/$defs/Unknown"}),
        json!({"anyOf": [{"type": "string"}, {"type": "null"}]}),
    ] {
        assert!(field("value", &schema, true).is_err(), "{schema}");
    }
    assert!(field("value", &json!({"type": "string", "default": ""}), false).is_err());
    let mut schema = catalog()[0].schema();
    schema["allOf"] = json!([]);
    assert!(fields(&schema).is_err());
    let collision = json!({
        "type": "object", "required": ["foo_bar", "fooBar"],
        "properties": {"foo_bar": {"type": "string"}, "fooBar": {"type": "string"}}
    });
    assert!(
        fields(&collision)
            .unwrap_err()
            .contains("duplicate Swift field")
    );
}

#[test]
fn nullable_fields_and_snake_case_names_preserve_the_wire_boundary() {
    let field = field(
        "fade_in",
        &json!({"type": ["integer", "null"], "format": "int64"}),
        false,
    )
    .unwrap();
    assert_eq!(field.wire, "fade_in");
    assert_eq!(field.swift, "`fadeIn`");
    assert_eq!(field.kind, "Int64");
    assert!(field.optional);
    assert!(identifier("bad-name").is_err());
    assert!(identifier("").is_err());
}

#[test]
fn freshness_check_rejects_missing_or_changed_files_without_overwriting() {
    let path = std::env::temp_dir().join(format!(
        "echode-swift-actions-{}.swift",
        echode_model::Id::generate()
    ));
    assert!(check(&path, "expected").is_err());
    std::fs::write(&path, "stale").unwrap();
    let error = check(&path, "expected").unwrap_err();
    assert!(error.contains("is stale; run make swift-actions"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "stale");
    std::fs::write(&path, "expected").unwrap();
    check(&path, "expected").unwrap();
    std::fs::remove_file(path).unwrap();
}
