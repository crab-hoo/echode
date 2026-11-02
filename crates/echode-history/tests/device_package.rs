//! Device package compatibility independent of the shared model field table.

use std::path::PathBuf;

use echode_actions::Command;
use echode_history::{DocumentHost, read_object, read_package, write_object, write_package};
use echode_model::{Id, Project};
use serde_json::{Value, json};

struct Package {
    root: PathBuf,
    track: PathBuf,
    project: Project,
    device: Id,
    state: String,
}

impl Package {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-device-package-{}", Id::generate()));
        let (project, track) = Project::new().add_track("device package");
        let device = Id::generate();
        let project = project
            .add_clap_device(track, device, "org.example.音源\"parameters\":[\\test")
            .unwrap();
        let state = write_object(&root, b"recorded plugin bytes").unwrap();
        write_package(&root, &project).unwrap();
        Self {
            track: root.join("tracks").join(format!("{track}.json")),
            root,
            project,
            device,
            state,
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
        assert_eq!(
            read_object(&self.root, &self.state).unwrap(),
            b"recorded plugin bytes"
        );
        self.project.check_invariants().unwrap();
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn device_package_keeps_scalar_order_layout_and_canonical_bytes() {
    let package = Package::new();
    // The masks bound this regression only, not the number of devices or pins.
    for mask in 0..8 {
        let mut project = package
            .project
            .set_device_bypassed(package.device, mask & 1 != 0)
            .unwrap()
            .set_device_offline(package.device, mask & 2 != 0)
            .unwrap()
            .set_device_state(
                package.device,
                (mask & 4 != 0).then(|| package.state.clone()),
            )
            .unwrap();
        for pin in [0, 17, u32::MAX - 1] {
            project = project
                .set_parameter_pinned(package.device, pin, true)
                .unwrap();
        }
        write_package(&package.root, &project).unwrap();
        let bytes = std::fs::read(&package.track).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let expected = project.tracks()[0].devices.iter().map(|device| {
            format!(
                "    {{\"id\":\"{}\",\"kind\":\"{}\",\"plugin\":{},\"state\":{},\"bypassed\":{},\"offline\":{},\"pinned\":{},\"parameters\":[",
                device.id,
                device.kind.as_str(),
                serde_json::to_string(&device.plugin).unwrap(),
                serde_json::to_string(&device.state).unwrap(),
                device.bypassed,
                device.offline,
                serde_json::to_string(&device.pinned).unwrap(),
            )
        }).collect::<Vec<_>>();
        let actual = text
            .lines()
            .filter(|line| line.starts_with("    {\"id\":"))
            .collect::<Vec<_>>();
        assert_eq!(
            actual, expected,
            "device package scalar order or spelling changed: {mask}"
        );
        assert!(
            text.contains("\"parameters\":[\n    ]},\n"),
            "empty parameter layout changed"
        );
        assert_eq!(package.track_json()["schema"], 6);
        let reopened = read_package(&package.root).unwrap();
        assert_eq!(reopened, project, "mask {mask}");
        write_package(&package.root, &reopened).unwrap();
        assert_eq!(std::fs::read(&package.track).unwrap(), bytes, "mask {mask}");
    }
}

#[test]
fn device_package_rejects_bad_shapes_and_preserves_optional_text_defaults() {
    let package = Package::new();
    let original = package.track_json();
    for field in ["id", "kind", "bypassed", "offline", "pinned", "parameters"] {
        let mut track = original.clone();
        track["devices"][0].as_object_mut().unwrap().remove(field);
        package.reject(&track, &format!("missing field `{field}`"), field);
        for wrong in [Value::Null, json!(42), json!({})] {
            let mut track = original.clone();
            track["devices"][0][field] = wrong;
            package.reject(&track, "invalid type", field);
        }
    }
    for (field, wrong, diagnostic) in [
        ("id", json!("not-an-id"), "invalid id not-an-id"),
        ("kind", json!("future"), "unknown device kind future"),
        ("plugin", json!(false), "invalid type"),
        ("state", json!([]), "invalid type"),
        ("bypassed", json!("true"), "invalid type"),
        ("offline", json!("false"), "invalid type"),
        ("pinned", json!([-1]), "invalid value"),
        ("pinned", json!([4294967296u64]), "invalid value"),
        ("pinned", json!(["17"]), "invalid type"),
        ("future", json!(true), "unknown field `future`"),
    ] {
        let mut track = original.clone();
        track["devices"][0][field] = wrong;
        package.reject(&track, diagnostic, field);
    }
    // Optional text was already accepted as absent, not just as explicit null.
    let mut track = original.clone();
    track["devices"][0].as_object_mut().unwrap().remove("state");
    track["devices"][1].as_object_mut().unwrap().remove("state");
    track["devices"][1]
        .as_object_mut()
        .unwrap()
        .remove("plugin");
    std::fs::write(&package.track, serde_json::to_vec(&track).unwrap()).unwrap();
    assert_eq!(read_package(&package.root).unwrap(), package.project);
}

#[test]
fn device_package_keeps_semantic_validation_and_state_object_checks() {
    let package = Package::new();
    let original = package.track_json();
    for (index, field, wrong, diagnostic) in [
        (0, "plugin", Value::Null, "does not match its plugin id"),
        (0, "plugin", json!(""), "does not match its plugin id"),
        (
            0,
            "plugin",
            json!("bad\nplugin"),
            "does not match its plugin id",
        ),
        (
            0,
            "state",
            json!(""),
            "state hash must be 64 lowercase hex digits",
        ),
        (
            0,
            "state",
            json!("A".repeat(64)),
            "state hash must be 64 lowercase hex digits",
        ),
        (0, "state", json!("0".repeat(64)), "object"),
        (0, "pinned", json!([17, 0]), "device cannot be removed"),
        (0, "pinned", json!([17, 17]), "device cannot be removed"),
        (0, "pinned", json!([u32::MAX]), "device cannot be removed"),
        (
            1,
            "plugin",
            json!("org.example.fx"),
            "does not match its plugin id",
        ),
        (1, "state", json!(package.state), "mixer state must be null"),
        (1, "bypassed", json!(true), "outside its range"),
        (1, "offline", json!(true), "outside its range"),
        (1, "pinned", json!([17]), "outside its range"),
    ] {
        let mut track = original.clone();
        track["devices"][index][field] = wrong;
        serde_json::from_value::<echode_model::Device>(track["devices"][index].clone()).unwrap();
        package.reject(&track, diagnostic, field);
    }
    let mut track = original.clone();
    track["devices"][0]["parameters"] = track["devices"][1]["parameters"].clone();
    package.reject(&track, "does not match its plugin id", "CLAP parameters");
    let mut track = original.clone();
    track["devices"][1]["id"] = track["devices"][0]["id"].clone();
    package.reject(&track, "id is already used", "duplicate device identity");
    let mut track = original.clone();
    track["devices"].as_array_mut().unwrap().swap(0, 1);
    package.reject(
        &track,
        "device cannot be removed",
        "mixer must end the chain",
    );
    // A well-formed hash must still authenticate its stored bytes.
    track = original;
    track["devices"][0]["state"] = json!(package.state);
    let object = package.root.join("media/objects").join(&package.state);
    std::fs::write(&object, b"corrupted").unwrap();
    std::fs::write(&package.track, serde_json::to_vec(&track).unwrap()).unwrap();
    let error = read_package(&package.root).unwrap_err().to_string();
    assert!(error.contains("does not match its BLAKE3"), "{error}");
    assert_eq!(std::fs::read(object).unwrap(), b"corrupted");
}

#[test]
fn device_package_keeps_legacy_migrations_and_commit_recovery() {
    let package = Package::new();
    let original = package.track_json();
    for schema in 1..=6 {
        let mut track = original.clone();
        track["schema"] = json!(schema);
        for device in track["devices"].as_array_mut().unwrap() {
            let device = device.as_object_mut().unwrap();
            if schema < 2 && device["kind"] == "mixer" {
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
    for command in [
        Command::BypassDevice {
            device: package.device,
            bypassed: true,
        },
        Command::OfflineDevice {
            device: package.device,
            offline: true,
        },
        Command::PinParameter {
            device: package.device,
            parameter: 17,
            pinned: true,
        },
    ] {
        let before = host.snapshot();
        host.perform(command).unwrap();
        let after = host.snapshot();
        host.undo().unwrap();
        assert_eq!(host.snapshot(), before);
        host.redo().unwrap();
        assert_eq!(host.snapshot(), after);
    }
    assert!(
        host.commit_plugin_state(package.device, b"new recorded state")
            .unwrap()
    );
    let expected = host.snapshot();
    host.revisions().unwrap();
    assert_eq!(read_package(&package.root).unwrap(), *expected);
    drop(host);
    // Force reopening from the saved commit instead of the stale working files.
    write_package(&package.root, &package.project).unwrap();
    let reopened = DocumentHost::open(&package.root).unwrap();
    assert_eq!(reopened.snapshot(), expected);
}
