//! Note package compatibility independent of the shared model field table.

use std::path::PathBuf;

use echode_actions::Command;
use echode_history::{DocumentHost, read_package, write_package};
use echode_model::{ClipKind, Content, Id, Note, Placement, Project, Tick};
use serde_json::{Value, json};

struct Package {
    root: PathBuf,
    content: PathBuf,
    project: Project,
}

impl Package {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-note-package-{}", Id::generate()));
        let (project, _) = Project::new().add_track("note package");
        let mut track = (*project.tracks()[0]).clone();
        let content = Id::from_raw(1);
        track.clips = vec![Placement {
            id: Id::from_raw(2),
            content,
            lane: echode_model::Lane::Timeline,
            start: Tick::new(0),
            length: Tick::new(4 * 705_600),
            offset: Tick::new(0),
            kind: ClipKind::Fixed,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
        }];
        let mut link = track.clips[0].clone();
        link.id = Id::from_raw(3);
        link.start = Tick::new(8 * 705_600);
        link.offset = link.start; // Both windows retain content origin zero.
        track.clips.push(link);
        let project = Project::try_assemble(
            project.meter(),
            project.tempo().clone(),
            vec![Content {
                id: content,
                audio: None,
                takes: Vec::new(),
                notes: vec![
                    Note {
                        id: Id::from_raw(4),
                        start: Tick::new(0),
                        length: Tick::new(100),
                        pitch: 60,
                        velocity: 12_345,
                        release: 54_321,
                        articulation: None,
                    },
                    Note {
                        id: Id::from_raw(5),
                        start: Tick::new(123),
                        length: Tick::new(37),
                        pitch: 72,
                        velocity: 1,
                        release: u16::MAX,
                        articulation: None,
                    },
                ],
                expressions: Vec::new(),
            }],
            vec![track],
        )
        .unwrap();
        write_package(&root, &project).unwrap();
        Self {
            content: root.join("contents").join(format!("{content}.json")),
            root,
            project,
        }
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.content).unwrap()).unwrap()
    }

    fn with_notes(&self, notes: Vec<Note>) -> Project {
        let mut content = (*self.project.contents()[0]).clone();
        content.notes = notes;
        Project::try_assemble(
            self.project.meter(),
            self.project.tempo().clone(),
            vec![content],
            self.project
                .tracks()
                .iter()
                .map(|track| (**track).clone())
                .collect(),
        )
        .unwrap()
    }

    fn reject(&self, content: &Value, diagnostic: &str, case: &str) {
        let bytes = serde_json::to_vec(content).unwrap();
        let header = std::fs::read(self.root.join("project.json")).unwrap();
        std::fs::write(&self.content, &bytes).unwrap();
        let error = read_package(&self.root).unwrap_err().to_string();
        assert!(error.contains(diagnostic), "{case}: {error}");
        assert_eq!(std::fs::read(&self.content).unwrap(), bytes, "{case}");
        assert_eq!(
            std::fs::read(self.root.join("project.json")).unwrap(),
            header
        );
        self.project.check_invariants().unwrap();
    }

    fn check_bytes(&self, project: &Project) {
        write_package(&self.root, project).unwrap();
        let bytes = std::fs::read(&self.content).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let notes = &project.contents()[0].notes;
        let expected = notes.iter().enumerate().map(|(index, note)| {
            let comma = if index + 1 == notes.len() { "" } else { "," };
            format!(
                "    {{\"id\":\"{}\",\"start\":{},\"length\":{},\"pitch\":{},\"velocity\":{},\"release\":{},\"articulation\":null}}{comma}",
                note.id, note.start.get(), note.length.get(), note.pitch, note.velocity, note.release,
            )
        }).collect::<Vec<_>>();
        let actual = text
            .lines()
            .filter(|line| line.starts_with("    {"))
            .collect::<Vec<_>>();
        assert_eq!(
            actual, expected,
            "note key order, integer spelling, or one-line layout changed"
        );
        assert_eq!(self.json()["schema"], 6);
        assert!(text.ends_with("  ],\n  \"expressions\":[]\n}\n"));
        let reopened = read_package(&self.root).unwrap();
        assert_eq!(&reopened, project);
        write_package(&self.root, &reopened).unwrap();
        assert_eq!(std::fs::read(&self.content).unwrap(), bytes);
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn note_serde_preserves_shape_without_accepting_it_as_a_project() {
    let stored = json!({
        "id": Id::from_raw(u128::MAX),
        "start": i64::MIN,
        "length": i64::MAX,
        "pitch": u8::MAX,
        "velocity": u16::MAX,
        "release": 12_345,
        "articulation": Id::from_raw(90),
    });
    let note: Note = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(note.id, Id::from_raw(u128::MAX));
    assert_eq!(note.start, Tick::new(i64::MIN));
    assert_eq!(note.length, Tick::new(i64::MAX));
    assert_eq!(note.pitch, u8::MAX);
    assert_eq!((note.velocity, note.release), (u16::MAX, 12_345));
    assert_eq!(note.articulation, Some(Id::from_raw(90)));
    assert_eq!(serde_json::to_value(&note).unwrap(), stored);
    let package = Package::new();
    let mut content = package.json();
    content["notes"][0] = stored;
    package.reject(
        &content,
        "articulation is not accepted yet",
        "shape is not acceptance",
    );
}

#[test]
fn note_package_keeps_scalar_order_integers_and_canonical_bytes() {
    let package = Package::new();
    // These masks bound only this regression, not the number of stored notes.
    for mask in 0..32 {
        let mut notes = package.project.contents()[0].notes.clone();
        let note = &mut notes[0];
        if mask & 1 != 0 {
            note.start = Tick::new(-17);
        }
        if mask & 2 != 0 {
            note.length = Tick::new(211);
        }
        if mask & 4 != 0 {
            note.pitch = 127;
        }
        if mask & 8 != 0 {
            note.velocity = u16::MAX;
        }
        if mask & 16 != 0 {
            note.release = 0;
        }
        package.check_bytes(&package.with_notes(notes));
    }
    let mut notes = package.project.contents()[0].notes.clone();
    notes[0].start = Tick::new(i64::MIN);
    notes[0].length = Tick::new(1);
    notes[0].pitch = 0;
    notes[0].velocity = 0;
    notes[1].start = Tick::new(i64::MAX - 1);
    notes[1].length = Tick::new(1);
    package.check_bytes(&package.with_notes(notes));
    let mut notes = package.project.contents()[0].notes.clone();
    notes[0].length = Tick::new(i64::MAX);
    package.check_bytes(&package.with_notes(notes));
    package.check_bytes(&package.with_notes(Vec::new()));
}

#[test]
fn note_package_rejects_bad_shapes_and_preserves_optional_articulation() {
    let package = Package::new();
    let original = package.json();
    for field in ["id", "start", "length", "pitch", "velocity", "release"] {
        let mut content = original.clone();
        content["notes"][0].as_object_mut().unwrap().remove(field);
        package.reject(&content, &format!("missing field `{field}`"), field);
        for wrong in [Value::Null, json!(false), json!([]), json!(1.5)] {
            let mut content = original.clone();
            content["notes"][0][field] = wrong;
            package.reject(&content, "invalid type", field);
        }
    }
    for (field, wrong, diagnostic) in [
        ("id", json!("bad-id"), "invalid id bad-id"),
        ("start", json!(u64::MAX), "invalid value"),
        ("length", json!(u64::MAX), "invalid value"),
        ("pitch", json!(-1), "invalid value"),
        ("pitch", json!(256), "invalid value"),
        ("velocity", json!(-1), "invalid value"),
        ("velocity", json!(65_536), "invalid value"),
        ("release", json!(-1), "invalid value"),
        ("release", json!(65_536), "invalid value"),
        ("articulation", json!(false), "invalid type"),
        ("articulation", json!("bad-id"), "invalid id bad-id"),
        ("future", json!(0), "unknown field `future`"),
    ] {
        let mut content = original.clone();
        content["notes"][0][field] = wrong;
        package.reject(&content, diagnostic, field);
    }
    let mut content = original;
    content["notes"][0]
        .as_object_mut()
        .unwrap()
        .remove("articulation");
    std::fs::write(&package.content, serde_json::to_vec(&content).unwrap()).unwrap();
    assert_eq!(read_package(&package.root).unwrap(), package.project);
}

#[test]
fn note_package_rejects_invalid_notes_order_ids_and_unsupported_data() {
    let package = Package::new();
    let original = package.json();
    for (field, wrong, diagnostic) in [
        ("pitch", json!(128), "is not usable"),
        ("pitch", json!(255), "is not usable"),
        ("length", json!(0), "is not usable"),
        ("length", json!(-1), "is not usable"),
        ("start", json!(i64::MAX), "not usable"),
        (
            "articulation",
            json!(Id::from_raw(90)),
            "articulation is not accepted yet",
        ),
    ] {
        let mut content = original.clone();
        content["notes"][0][field] = wrong;
        package.reject(&content, diagnostic, field);
    }
    let mut content = original.clone();
    content["notes"][1]["id"] = content["notes"][0]["id"].clone();
    package.reject(
        &content,
        "id is already used in the project",
        "duplicate note id",
    );
    content = original.clone();
    content["notes"][0]["id"] = content["id"].clone();
    package.reject(
        &content,
        "id is already used in the project",
        "cross-object id",
    );
    content = original.clone();
    content["notes"].as_array_mut().unwrap().reverse();
    package.reject(&content, "not in start, pitch, id order", "note order");
    content = original;
    content["expressions"] = json!([{
        "note": Id::from_raw(4), "kind": "pitch", "points": [{"at": 0, "value": 1}]
    }]);
    package.reject(
        &content,
        "expressions are not accepted yet",
        "expression curve",
    );
}

#[test]
fn note_package_keeps_legacy_migrations_shared_content_and_commit_recovery() {
    let package = Package::new();
    let original = package.json();
    for schema in 1..=6 {
        let mut content = original.clone();
        content["schema"] = json!(schema);
        std::fs::write(&package.content, serde_json::to_vec(&content).unwrap()).unwrap();
        assert_eq!(
            read_package(&package.root).unwrap(),
            package.project,
            "schema {schema}"
        );
    }
    write_package(&package.root, &package.project).unwrap();
    let track = &package.project.tracks()[0];
    let note = &package.project.contents()[0].notes[0];
    let host = DocumentHost::open(&package.root).unwrap();
    host.perform(Command::DuplicateNote {
        track: track.id,
        clip: track.clips[0].id,
        note: note.id,
        copy: Some(Id::from_raw(6)),
        pitch: 64,
        start: Tick::new(456),
    })
    .unwrap();
    let expected = host.snapshot();
    let copied = expected.contents()[0]
        .notes
        .iter()
        .find(|note| note.id == Id::from_raw(6))
        .unwrap();
    assert_eq!((copied.velocity, copied.release), (12_345, 54_321));
    assert_eq!(
        expected.tracks()[0].clips[0].content,
        expected.tracks()[0].clips[1].content
    );
    host.undo().unwrap();
    assert_eq!(*host.snapshot(), package.project);
    host.redo().unwrap();
    assert_eq!(host.snapshot(), expected);
    host.revisions().unwrap();
    assert_eq!(read_package(&package.root).unwrap(), *expected);
    drop(host);
    // Force the product host to recover the commit, not the stale working files.
    write_package(&package.root, &package.project).unwrap();
    let reopened = DocumentHost::open(&package.root).unwrap();
    assert_eq!(reopened.snapshot(), expected);
}
