//! Package parameter compatibility, independent of the model's field table.

use std::path::PathBuf;

use echode_actions::Command;
use echode_history::{DocumentHost, read_package, write_package};
use echode_model::{Id, Project};
use serde_json::{Value, json};

struct Package {
    root: PathBuf,
    track: PathBuf,
    project: Project,
}

impl Package {
    fn new() -> Self {
        let (project, track) = Project::new().add_track("parameter package");
        let root =
            std::env::temp_dir().join(format!("echode-parameter-package-{}", Id::generate()));
        write_package(&root, &project).unwrap();
        let track = root.join("tracks").join(format!("{track}.json"));
        Self {
            root,
            track,
            project,
        }
    }

    fn track_json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.track).unwrap()).unwrap()
    }

    fn reject(&self, track: &Value, diagnostic: &str, case: &str) {
        let bytes = serde_json::to_vec(track).unwrap();
        std::fs::write(&self.track, &bytes).unwrap();
        let error = read_package(&self.root).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{case}: {error}");
        assert_eq!(std::fs::read(&self.track).unwrap(), bytes, "{case}");
        self.project.check_invariants().unwrap();
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn parameter_package_keeps_schema_six_field_order_and_canonical_bytes() {
    let package = Package::new();
    let mut project = package.project.clone();
    let parameters = &package.project.tracks()[0].devices[0].parameters;
    for (parameter, value) in parameters.iter().zip([0.5, -0.5, 1.0, 0.0]) {
        project = project.set_parameter(parameter.id, value).unwrap();
    }
    write_package(&package.root, &project).unwrap();
    let bytes = std::fs::read(&package.track).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let expected: Vec<_> = parameters.iter().zip([
        ("gain", "0", "4", "1", "0.5"),
        ("pan", "-1", "1", "0", "-0.5"),
        ("mute", "0", "1", "0", "1"),
        ("solo", "0", "1", "0", "0"),
    ]).map(|(parameter, (key, min, max, default, value))| {
        format!(
            "      {{\"id\":\"{}\",\"key\":\"{key}\",\"min\":{min},\"max\":{max},\"default\":{default},\"value\":{value}}}",
            parameter.id,
        )
    }).collect();
    let actual = text
        .lines()
        .filter(|line| line.starts_with("      {\"id\":"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        actual,
        expected.join(",\n"),
        "parameter package field order or spelling changed"
    );
    assert_eq!(package.track_json()["schema"], 6);
    let reopened = read_package(&package.root).unwrap();
    assert_eq!(reopened, project);
    write_package(&package.root, &reopened).unwrap();
    assert_eq!(std::fs::read(&package.track).unwrap(), bytes);
}

#[test]
fn parameter_package_rejects_missing_mistyped_and_unknown_fields_without_writing() {
    let package = Package::new();
    let original = package.track_json();
    for field in ["id", "key", "min", "max", "default", "value"] {
        let mut track = original.clone();
        track["devices"][0]["parameters"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        package.reject(&track, &format!("missing field `{field}`"), field);
        for wrong in [Value::Null, json!(false), json!([]), json!({})] {
            let mut track = original.clone();
            track["devices"][0]["parameters"][0][field] = wrong;
            package.reject(&track, "invalid type", field);
        }
    }
    let mut track = original.clone();
    track["devices"][0]["parameters"][0]["future"] = json!(42);
    package.reject(&track, "unknown field `future`", "unknown parameter field");
    for (field, wrong, diagnostic) in [
        ("id", json!("not-an-id"), "invalid id not-an-id"),
        ("key", json!("future"), "unknown parameter future"),
        ("min", json!("0"), "invalid type"),
        ("max", json!([4]), "invalid type"),
        ("default", json!("1"), "invalid type"),
        ("value", json!("0.5"), "invalid type"),
    ] {
        let mut track = original.clone();
        track["devices"][0]["parameters"][0][field] = wrong;
        package.reject(&track, diagnostic, field);
    }
}

#[test]
fn parameter_package_keeps_invariant_validation_after_decoding() {
    let package = Package::new();
    let original = package.track_json();
    for (field, wrong) in [
        ("min", json!(-1)),
        ("max", json!(5)),
        ("default", json!(0)),
        ("value", json!(4.5)),
        ("key", json!("pan")),
    ] {
        let mut track = original.clone();
        track["devices"][0]["parameters"][0][field] = wrong;
        package.reject(&track, "outside its range", field);
    }
    let mut track = original.clone();
    track["devices"][0]["parameters"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    package.reject(&track, "outside its range", "parameter order");
    let mut track = original.clone();
    track["devices"][0]["parameters"][1]["id"] = track["devices"][0]["parameters"][0]["id"].clone();
    package.reject(&track, "id is already used", "duplicate parameter identity");
}

#[test]
fn parameter_package_keeps_legacy_migrations_and_commit_recovery() {
    let package = Package::new();
    let original = package.track_json();
    for schema in 1..=6 {
        let mut track = original.clone();
        track["schema"] = json!(schema);
        let device = track["devices"][0].as_object_mut().unwrap();
        if schema < 2 {
            device.remove("plugin");
        }
        if schema < 4 {
            device.remove("bypassed");
        }
        if schema < 5 {
            device.remove("offline");
        }
        if schema < 6 {
            device.remove("pinned");
        }
        std::fs::write(&package.track, serde_json::to_vec(&track).unwrap()).unwrap();
        assert_eq!(
            read_package(&package.root).unwrap(),
            package.project,
            "schema {schema}"
        );
    }
    write_package(&package.root, &package.project).unwrap();
    let host = DocumentHost::open(&package.root).unwrap();
    for (parameter, value) in package.project.tracks()[0].devices[0]
        .parameters
        .iter()
        .zip([0.5, -0.5, 1.0, 1.0])
    {
        let before = host.snapshot();
        host.perform(Command::SetParameter {
            parameter: parameter.id,
            value,
        })
        .unwrap();
        let after = host.snapshot();
        host.undo().unwrap();
        assert_eq!(host.snapshot(), before);
        host.redo().unwrap();
        assert_eq!(host.snapshot(), after);
    }
    let expected = host.snapshot();
    host.revisions().unwrap();
    assert_eq!(read_package(&package.root).unwrap(), *expected);
    drop(host);
    // Force reopening to read the saved commit instead of the working files.
    write_package(&package.root, &package.project).unwrap();
    let reopened = DocumentHost::open(&package.root).unwrap();
    assert_eq!(reopened.snapshot(), expected);
}
