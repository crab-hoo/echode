//! Independent device field expectations at the public changeset and journal boundary.

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_model::{
    Change, ChangeSet, Field, Id, ModelError, ObjectKey, ObjectKind, Project, Property, Value,
};
use proptest::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Settings {
    bypassed: bool,
    offline: bool,
    pinned: Vec<u32>,
    state: Option<String>,
}

fn settings() -> impl Strategy<Value = Settings> {
    (
        any::<bool>(),
        any::<bool>(),
        // Collection length and case counts bound this regression only, not devices.
        prop::collection::btree_set(0u32..u32::MAX, 0..12),
        proptest::option::of(any::<[u8; 32]>()),
    )
        .prop_map(|(bypassed, offline, pinned, state)| Settings {
            bypassed,
            offline,
            pinned: pinned.into_iter().collect(),
            state: state.map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
        })
}

fn configure(project: &Project, id: Id, settings: &Settings) -> Project {
    let mut project = project
        .set_device_bypassed(id, settings.bypassed)
        .unwrap()
        .set_device_offline(id, settings.offline)
        .unwrap()
        .set_device_state(id, settings.state.clone())
        .unwrap();
    for parameter in &settings.pinned {
        project = project.set_parameter_pinned(id, *parameter, true).unwrap();
    }
    project.check_invariants().unwrap();
    project
}

fn replay(project: &Project, changes: &ChangeSet) -> Project {
    let line = JournalLine {
        seq: 1,
        op: JournalOp::Edit,
        actions: Vec::new(),
        changes: changes.clone(),
    };
    let decoded = decode_line(&encode_line(&line)).unwrap();
    assert_eq!(decoded, line);
    project.apply_changeset(&decoded.changes).unwrap()
}

fn creation(changes: &ChangeSet, id: Id) -> Vec<Field> {
    changes
        .changes()
        .iter()
        .find_map(|change| match change {
            Change::Create {
                kind: ObjectKind::Device,
                id: ObjectKey::Id(found),
                fields,
            } if *found == id => Some(fields.clone()),
            _ => None,
        })
        .unwrap()
}

fn check_roundtrip(before: Settings, after: Settings) {
    let empty = Project::new();
    let (base, track) = empty.add_track("device fields");
    let id = Id::generate();
    let base = base.add_clap_device(track, id, "org.example.音源").unwrap();
    let before_project = configure(&base, id, &before);
    let after_project = configure(&base, id, &after);
    for (project, expected) in [(&before_project, &before), (&after_project, &after)] {
        let device = &project.tracks()[0].devices[0];
        assert_eq!(device.bypassed, expected.bypassed, "bypass crossed fields");
        assert_eq!(device.offline, expected.offline, "offline crossed fields");
        assert_eq!(device.pinned, expected.pinned, "pins crossed fields");
        assert_eq!(device.state, expected.state, "state crossed fields");
    }
    let changes = before_project.diff(&after_project);
    let expected: Vec<_> = [
        (
            Property::PinnedParameters,
            Value::ParamIds(before.pinned),
            Value::ParamIds(after.pinned.clone()),
        ),
        (
            Property::Offline,
            Value::Byte(u8::from(before.offline)),
            Value::Byte(u8::from(after.offline)),
        ),
        (
            Property::Bypassed,
            Value::Byte(u8::from(before.bypassed)),
            Value::Byte(u8::from(after.bypassed)),
        ),
        (
            Property::PluginState,
            Value::Text(before.state.unwrap_or_default()),
            Value::Text(after.state.clone().unwrap_or_default()),
        ),
    ]
    .into_iter()
    .filter(|(_, before, after)| before != after)
    .map(|(property, before, after)| Change::Update {
        id: ObjectKey::Id(id),
        property,
        before,
        after,
    })
    .collect();
    assert_eq!(changes.changes(), expected, "device update wire order");
    assert_eq!(replay(&before_project, &changes), after_project);
    assert_eq!(replay(&after_project, &changes.invert()), before_project);
    assert_eq!(changes.invert().invert(), changes);
    assert!(after_project.diff(&after_project).is_empty());

    let created = empty.diff(&after_project);
    let mut expected_fields = vec![
        field(Property::Owner, Value::Id(track)),
        field(Property::DeviceKind, Value::Text("clap".into())),
        field(Property::Bypassed, Value::Byte(u8::from(after.bypassed))),
        field(Property::Offline, Value::Byte(u8::from(after.offline))),
        field(Property::PinnedParameters, Value::ParamIds(after.pinned)),
        field(Property::Plugin, Value::Text("org.example.音源".into())),
    ];
    if let Some(state) = after.state {
        expected_fields.push(field(Property::PluginState, Value::Text(state)));
    }
    expected_fields.push(field(Property::ParamOrder, Value::Ids(Vec::new())));
    assert_eq!(
        creation(&created, id),
        expected_fields,
        "device creation wire order"
    );
    assert_eq!(replay(&empty, &created), after_project);
    assert_eq!(replay(&after_project, &created.invert()), empty);
    let deleted = after_project.diff(&empty);
    assert_eq!(replay(&after_project, &deleted), empty);
    assert_eq!(replay(&empty, &deleted.invert()), after_project);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn device_changesets_preserve_independent_settings(before in settings(), after in settings()) {
        check_roundtrip(before, after);
    }
}

#[test]
fn device_changesets_keep_flags_pins_and_state_independent() {
    let base = Settings {
        bypassed: false,
        offline: false,
        pinned: Vec::new(),
        state: None,
    };
    for bit in 0..16 {
        let after = Settings {
            bypassed: bit & 1 != 0,
            offline: bit & 2 != 0,
            pinned: if bit & 4 != 0 {
                vec![0, 7, u32::MAX - 1]
            } else {
                Vec::new()
            },
            state: (bit & 8 != 0).then(|| "0123456789abcdef".repeat(4)),
        };
        check_roundtrip(base.clone(), after.clone());
        check_roundtrip(after.clone(), base.clone());
        check_roundtrip(after.clone(), after);
    }
}

fn field(property: Property, value: Value) -> Field {
    Field { property, value }
}

fn replace_creation(changes: &ChangeSet, id: Id, fields: Vec<Field>) -> ChangeSet {
    let mut changed = ChangeSet::new();
    for change in changes.changes() {
        changed.push(match change {
            Change::Create {
                kind: ObjectKind::Device,
                id: ObjectKey::Id(found),
                ..
            } if *found == id => Change::Create {
                kind: ObjectKind::Device,
                id: ObjectKey::Id(id),
                fields: fields.clone(),
            },
            _ => change.clone(),
        });
    }
    changed
}

fn set(id: Id, property: Property, after: Value) -> ChangeSet {
    let mut changes = ChangeSet::new();
    changes.push(Change::Update {
        id: ObjectKey::Id(id),
        property,
        before: after.clone(),
        after,
    });
    changes
}

fn without(fields: &[Field], property: Property) -> Vec<Field> {
    fields
        .iter()
        .filter(|field| field.property != property)
        .cloned()
        .collect()
}

fn assert_invalid(project: &Project, changes: &ChangeSet, context: impl std::fmt::Debug) {
    assert_eq!(
        project.apply_changeset(changes),
        Err(ModelError::InvalidChange),
        "{context:?}"
    );
}

#[test]
fn device_changesets_keep_legacy_creation_defaults_and_validate_fields() {
    let empty = Project::new();
    let (base, track) = empty.add_track("legacy device");
    let id = Id::generate();
    let base = base.add_clap_device(track, id, "org.example.fx").unwrap();
    let created = empty.diff(&base);
    let fields = creation(&created, id);
    for property in [
        Property::Bypassed,
        Property::Offline,
        Property::PinnedParameters,
    ] {
        let legacy = without(&fields, property);
        assert_eq!(
            replay(&empty, &replace_creation(&created, id, legacy)),
            base,
            "{property:?} legacy default"
        );
    }
    let legacy = without(&fields, Property::Bypassed);
    let legacy = without(&legacy, Property::Offline);
    let legacy = without(&legacy, Property::PinnedParameters);
    assert_eq!(
        replay(&empty, &replace_creation(&created, id, legacy)),
        base
    );

    for property in [Property::Owner, Property::DeviceKind, Property::ParamOrder] {
        assert_invalid(
            &empty,
            &replace_creation(&created, id, without(&fields, property)),
            ("missing", property),
        );
    }
    let invalid = [
        (Property::Bypassed, Value::Byte(2)),
        (Property::Offline, Value::Byte(255)),
        (Property::PinnedParameters, Value::ParamIds(vec![1, 1])),
        (Property::PinnedParameters, Value::ParamIds(vec![7, 0])),
        (Property::PinnedParameters, Value::ParamIds(vec![u32::MAX])),
        (Property::PluginState, Value::Text("A".repeat(64))),
        (Property::PluginState, Value::Text("0".repeat(63))),
        (Property::PluginState, Value::Text("g".repeat(64))),
    ];
    let wrong_types = [
        Property::Owner,
        Property::DeviceKind,
        Property::Bypassed,
        Property::Offline,
        Property::PinnedParameters,
        Property::Plugin,
        Property::PluginState,
        Property::ParamOrder,
    ]
    .map(|property| (property, Value::Tick(echode_model::Tick::new(0))));
    for (property, value) in invalid.into_iter().chain(wrong_types) {
        let mut malformed = without(&fields, property);
        malformed.push(field(property, value.clone()));
        assert_invalid(
            &empty,
            &replace_creation(&created, id, malformed),
            ("creation", property, &value),
        );
        assert_invalid(
            &base,
            &set(id, property, value.clone()),
            ("update", property, &value),
        );
    }
    for property in [Property::Plugin, Property::PluginState] {
        let mut duplicate = fields.clone();
        duplicate.push(field(property, Value::Text("0".repeat(64))));
        duplicate.push(field(property, Value::Text("1".repeat(64))));
        assert_invalid(
            &empty,
            &replace_creation(&created, id, duplicate),
            ("duplicate", property),
        );
    }
    let mixer = base.tracks()[0].devices[1].id;
    for (target, property, value) in [
        (Id::generate(), Property::Offline, Value::Byte(1)),
        (id, Property::Owner, Value::Id(track)),
        (id, Property::DeviceKind, Value::Text("clap".into())),
        (mixer, Property::Bypassed, Value::Byte(1)),
        (mixer, Property::Offline, Value::Byte(1)),
        (mixer, Property::PinnedParameters, Value::ParamIds(vec![0])),
        (mixer, Property::PluginState, Value::Text("0".repeat(64))),
    ] {
        assert_invalid(&base, &set(target, property, value), (target, property));
    }
    // Creation rejects an empty hash; updating to empty text clears a saved state.
    let mut empty_hash = fields.clone();
    empty_hash.push(field(Property::PluginState, Value::Text(String::new())));
    assert_invalid(
        &empty,
        &replace_creation(&created, id, empty_hash),
        "empty creation hash",
    );
    assert_eq!(
        base.apply_changeset(&set(id, Property::PluginState, Value::Text(String::new())))
            .unwrap(),
        base
    );
    let renamed = base
        .apply_changeset(&set(
            id,
            Property::Plugin,
            Value::Text("org.example.legacy".into()),
        ))
        .unwrap();
    assert_eq!(
        renamed.tracks()[0].devices[0].plugin.as_deref(),
        Some("org.example.legacy")
    );
    base.check_invariants().unwrap();
}
