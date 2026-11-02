//! Track package compatibility independent of the shared scalar field table.

use std::path::PathBuf;

use echode_actions::Command;
use echode_history::{DocumentHost, read_package, write_package};
use echode_model::{Id, NoteDraft, Project, Tick, Track, TrackMetadata};
use serde_json::{Value, json};

struct Package {
    root: PathBuf,
    project: Project,
    folder: Id,
    branch: Id,
    target: Id,
    other: Id,
}

impl Package {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-track-package-{}", Id::generate()));
        let (project, folder) = Project::new().add_track("folder");
        let (project, branch) = project.add_child(folder, "nested folder").unwrap();
        let (project, target) = project.add_child(branch, "target").unwrap();
        let (project, other) = project.add_track("other folder");
        let project = project
            .add_clap_device(target, Id::generate(), "org.example.fixture")
            .unwrap();
        let (project, _) = project
            .insert_note(
                target,
                NoteDraft {
                    pitch: 60,
                    velocity: 97,
                    start: Tick::new(123),
                    length: Tick::new(456),
                },
            )
            .unwrap();
        write_package(&root, &project).unwrap();
        Self {
            root,
            project,
            folder,
            branch,
            target,
            other,
        }
    }

    fn path(&self, id: Id) -> PathBuf {
        self.root.join("tracks").join(format!("{id}.json"))
    }

    fn json(&self, id: Id) -> Value {
        serde_json::from_slice(&std::fs::read(self.path(id)).unwrap()).unwrap()
    }

    fn reject(&self, id: Id, value: &Value, diagnostic: &str, case: &str) {
        let bytes = serde_json::to_vec(value).unwrap();
        std::fs::write(self.path(id), &bytes).unwrap();
        let before = self.bytes();
        let error = read_package(&self.root).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{case}: {error}");
        assert_eq!(
            self.bytes(),
            before,
            "{case}: rejection changed the package"
        );
        write_package(&self.root, &self.project).unwrap();
    }

    fn bytes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut paths = vec![self.root.join("project.json")];
        paths.extend(
            self.project
                .tracks()
                .iter()
                .map(|track| self.path(track.id)),
        );
        paths.extend(self.project.contents().iter().map(|content| {
            self.root
                .join("contents")
                .join(format!("{}.json", content.id))
        }));
        paths
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    fn check_bytes(&self, project: &Project) {
        write_package(&self.root, project).unwrap();
        for track in project.tracks() {
            let text = std::fs::read_to_string(self.path(track.id)).unwrap();
            let parent = track
                .parent
                .map_or_else(|| "null".to_owned(), |id| format!("\"{id}\""));
            let expected = format!(
                "{{\n  \"schema\":6,\n  \"id\":\"{}\",\n  \"name\":{},\n  \"parent\":{},\n  \"order\":{},\n  \"devices\":[\n",
                track.id,
                serde_json::to_string(&track.name).unwrap(),
                parent,
                serde_json::to_string(&track.order).unwrap(),
            );
            assert!(
                text.starts_with(&expected),
                "track scalar order or spelling changed: {text}"
            );
            assert!(text.ends_with("  ]\n}\n"));
        }
        let bytes = self.bytes();
        let reopened = read_package(&self.root).unwrap();
        assert_eq!(&reopened, project);
        write_package(&self.root, &reopened).unwrap();
        assert_eq!(
            self.bytes(),
            bytes,
            "canonical package bytes changed on resave"
        );
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn track(project: &Project, id: Id) -> &Track {
    project
        .tracks()
        .iter()
        .find(|track| track.id == id)
        .unwrap()
}

#[test]
fn track_metadata_decodes_shape_without_validating_a_project() {
    let stored = json!({
        "name": "音源\n\"devices\":[\\test",
        "parent": Id::from_raw(u128::MAX),
        "order": "not a fractional key",
    });
    let metadata: TrackMetadata = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(metadata.name, "音源\n\"devices\":[\\test");
    assert_eq!(metadata.parent, Some(Id::from_raw(u128::MAX)));
    assert_eq!(metadata.order, "not a fractional key");
    assert_eq!(serde_json::to_value(&metadata).unwrap(), stored);
    let package = Package::new();
    let original = track(&package.project, package.target);
    let restored = TrackMetadata::from(original).into_track(
        original.id,
        original.clips.clone(),
        original.devices.clone(),
    );
    assert_eq!(&restored, original);
    let mut value = package.json(package.target);
    for field in ["name", "parent", "order"] {
        value[field] = stored[field].clone();
    }
    package.reject(
        package.target,
        &value,
        "track is not in the project",
        "shape is not project acceptance",
    );
}

#[test]
fn track_package_keeps_scalar_order_escaping_hierarchy_and_children() {
    let package = Package::new();
    let (keys, extra) = package.project.add_track("key source only");
    let orders = [
        track(&package.project, package.other).order.clone(),
        track(&keys, extra).order.clone(),
    ];
    for name in ["", "音源\n\"parent\":null,\\\t\u{0000}", "folder 🦀"] {
        for parent in [Some(package.branch), Some(package.other), None] {
            for order in &orders {
                let project = package.project.set_parent(package.target, parent).unwrap();
                let mut tracks = project
                    .tracks()
                    .iter()
                    .map(|track| (**track).clone())
                    .collect::<Vec<_>>();
                let target = tracks
                    .iter_mut()
                    .find(|track| track.id == package.target)
                    .unwrap();
                target.name = name.to_owned();
                // At the root retain the unique key assigned by set_parent.
                if parent.is_some() {
                    target.order = order.clone();
                }
                let project = Project::try_assemble(
                    project.meter(),
                    project.tempo().clone(),
                    project
                        .contents()
                        .iter()
                        .map(|content| (**content).clone())
                        .collect(),
                    tracks,
                )
                .unwrap();
                package.check_bytes(&project);
                assert_eq!(
                    track(&project, package.target).clips,
                    track(&package.project, package.target).clips
                );
                assert_eq!(
                    track(&project, package.target).devices,
                    track(&package.project, package.target).devices
                );
            }
        }
    }
}

#[test]
fn track_package_rejects_missing_wrong_and_unknown_fields_without_repair() {
    let package = Package::new();
    let original = package.json(package.target);
    for field in ["id", "name", "order", "devices", "clips"] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(field);
        package.reject(package.target, &value, "missing field", field);
    }
    for (field, wrong, diagnostic) in [
        ("id", json!(17), "invalid type"),
        ("id", json!("invalid"), "invalid"),
        ("name", Value::Null, "invalid type"),
        ("name", json!(["name"]), "invalid type"),
        ("parent", json!(false), "invalid type"),
        ("parent", json!("invalid"), "invalid id"),
        ("order", json!(17), "invalid type"),
        ("order", Value::Null, "invalid type"),
        ("devices", json!({}), "invalid type"),
        ("clips", json!({}), "invalid type"),
    ] {
        let mut value = original.clone();
        value[field] = wrong;
        package.reject(package.target, &value, diagnostic, field);
    }
    let mut value = original.clone();
    value["unexpected"] = json!(true);
    package.reject(package.target, &value, "unknown field", "unknown scalar");
    value = original;
    value["devices"][0]["unexpected"] = json!(true);
    package.reject(
        package.target,
        &value,
        "unknown field",
        "unknown child scalar",
    );
    let mut root = package.json(package.folder);
    root.as_object_mut().unwrap().remove("parent");
    std::fs::write(
        package.path(package.folder),
        serde_json::to_vec(&root).unwrap(),
    )
    .unwrap();
    assert_eq!(
        read_package(&package.root).unwrap(),
        package.project,
        "omitted optional parent changed"
    );
    // Parent references retain the existing UUID parser's noncanonical input support.
    let mut value = package.json(package.target);
    value["parent"] = json!(package.branch.to_string().replace('-', "").to_uppercase());
    std::fs::write(
        package.path(package.target),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert_eq!(read_package(&package.root).unwrap(), package.project);
}

#[test]
fn track_package_rejects_invalid_tree_keys_identity_and_children() {
    let package = Package::new();
    let original = package.json(package.target);
    for (field, wrong, diagnostic, case) in [
        (
            "parent",
            json!(Id::generate()),
            "track is not in the project",
            "missing parent",
        ),
        (
            "parent",
            json!(package.target),
            "cannot contain itself",
            "self parent",
        ),
        (
            "order",
            json!("not a fractional key"),
            "track order key",
            "invalid key",
        ),
        ("order", json!(""), "track order key", "empty key"),
        (
            "id",
            json!(package.other),
            "does not contain that id",
            "filename identity",
        ),
        (
            "devices",
            json!([]),
            "plugin id is unusable",
            "missing mixer",
        ),
    ] {
        let mut value = original.clone();
        value[field] = wrong;
        package.reject(package.target, &value, diagnostic, case);
    }
    let mut root = package.json(package.folder);
    root["parent"] = json!(package.target);
    package.reject(
        package.folder,
        &root,
        "cannot contain itself",
        "ancestor cycle",
    );
    root = package.json(package.other);
    root["order"] = package.json(package.folder)["order"].clone();
    package.reject(
        package.other,
        &root,
        "track order key",
        "duplicate sibling key",
    );
    let mut value = original.clone();
    value["clips"][0]["content"] = json!(Id::generate());
    package.reject(
        package.target,
        &value,
        "clip is not on the track",
        "missing content",
    );
    value = original.clone();
    value["devices"][0]["id"] = value["devices"][1]["id"].clone();
    package.reject(
        package.target,
        &value,
        "id is already used",
        "duplicate child id",
    );
    value = original;
    value["clips"][0]["id"] = json!(package.target);
    package.reject(
        package.target,
        &value,
        "id is already used",
        "cross-object identity",
    );
}

#[test]
fn track_package_keeps_legacy_migrations_and_subtree_history_recovery() {
    let package = Package::new();
    let original = package.json(package.target);
    for schema in 1..=6 {
        let mut value = original.clone();
        value["schema"] = json!(schema);
        for device in value["devices"].as_array_mut().unwrap() {
            let device = device.as_object_mut().unwrap();
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
        std::fs::write(
            package.path(package.target),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_package(&package.root).unwrap(),
            package.project,
            "schema {schema}"
        );
    }
    write_package(&package.root, &package.project).unwrap();
    let host = DocumentHost::open(&package.root).unwrap();
    for command in [
        Command::SetParent {
            track: package.branch,
            parent: Some(package.other),
        },
        Command::DuplicateTrack {
            source: package.branch,
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
    let expected = host.snapshot();
    host.revisions().unwrap();
    assert_eq!(read_package(&package.root).unwrap(), *expected);
    drop(host);
    // Force the product host to recover the commit rather than the working files.
    write_package(&package.root, &package.project).unwrap();
    let reopened = DocumentHost::open(&package.root).unwrap();
    assert_eq!(reopened.snapshot(), expected);
}
