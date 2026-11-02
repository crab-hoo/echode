//! Cross-boundary expectations deliberately independent of the field registries.
use std::path::PathBuf;

use echode_actions::{JournalLine, JournalOp, decode_line, encode_line};
use echode_history::{read_package, write_package};
use echode_model::{AudioRef, Id, Meter, Project, Tempo, Tick};
use serde_json::{Value, json};

struct Package(PathBuf);
impl Package {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("echode-schema-fields-{}", Id::generate())))
    }
}
impl Drop for Package {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> Project {
    let project = Project::with_signature(
        Meter::try_new(7, 8).unwrap(),
        Tempo::rational(241, 2).unwrap(),
    )
    .with_tempo(Tick::new(-123), Tempo::rational(271, 3).unwrap());
    let (project, track) = project.add_track("fields 日本語");
    let project = project
        .place_audio_with_fades(
            track,
            Id::generate(),
            Id::generate(),
            Tick::new(123),
            AudioRef::try_new("ab".repeat(32), 48_000, 2, 96_000).unwrap(),
            17,
            31,
        )
        .unwrap();
    let clip = project.tracks()[0].clips[0].id;
    project
        .set_clip_gain(clip, f32::from_bits(0x0000_0001))
        .unwrap()
}

#[test]
fn package_fields_keep_legacy_layout_and_replay_across_all_remaining_objects() {
    let package = Package::new();
    let original = fixture();
    write_package(&package.0, &original).unwrap();
    let project_text = std::fs::read_to_string(package.0.join("project.json")).unwrap();
    assert!(project_text.starts_with("{\n  \"schema\":6,\n  \"meter\":{\"numerator\":7,\"denominator\":8},\n  \"tempo\":[\n    {\"at\":-123,\"numerator\":271,\"denominator\":3},\n    {\"at\":0,\"numerator\":241,\"denominator\":2}\n  ],\n"));
    let track = &original.tracks()[0];
    let clip = &track.clips[0];
    let track_path = package.0.join("tracks").join(format!("{}.json", track.id));
    let track_text = std::fs::read_to_string(&track_path).unwrap();
    assert!(track_text.contains(&format!("    {{\"id\":\"{}\",\"content\":\"{}\",\"lane\":\"timeline\",\"start\":123,\"length\":{},\"offset\":0,\"kind\":\"fixed\",\"gain\":{},\"fade_in\":17,\"fade_out\":31}}\n", clip.id, clip.content, clip.length.get(), clip.gain)));
    let content_path = package
        .0
        .join("contents")
        .join(format!("{}.json", clip.content));
    let content_text = std::fs::read_to_string(&content_path).unwrap();
    assert_eq!(
        content_text,
        format!(
            "{{\n  \"schema\":6,\n  \"id\":\"{}\",\n  \"audio\":{{\"hash\":\"{}\",\"sample_rate\":48000,\"channels\":2,\"frames\":96000}},\n  \"takes\":[],\n  \"notes\":[\n  ],\n  \"expressions\":[]\n}}\n",
            clip.content,
            "ab".repeat(32)
        )
    );
    let loaded = read_package(&package.0).unwrap();
    assert_eq!(loaded, original);
    let edited = loaded
        .set_clip_fades(clip.id, 13, 29)
        .unwrap()
        .set_clip_gain(clip.id, 0.125)
        .unwrap()
        .with_tempo(Tick::new(127), Tempo::rational(301, 2).unwrap());
    let line = JournalLine {
        seq: 1,
        op: JournalOp::Edit,
        actions: Vec::new(),
        changes: loaded.diff(&edited),
    };
    let decoded = decode_line(&encode_line(&line)).unwrap();
    assert_eq!(decoded, line);
    let replayed = loaded.apply_changeset(&decoded.changes).unwrap();
    assert_eq!(replayed, edited);
    assert_eq!(
        replayed.apply_changeset(&decoded.changes.invert()).unwrap(),
        loaded
    );
    write_package(&package.0, &replayed).unwrap();
    assert_eq!(read_package(&package.0).unwrap(), edited);
    write_package(&package.0, &original).unwrap();
    assert_eq!(std::fs::read_to_string(track_path).unwrap(), track_text);
    assert_eq!(std::fs::read_to_string(content_path).unwrap(), content_text);
    assert_eq!(
        std::fs::read_to_string(package.0.join("project.json")).unwrap(),
        project_text
    );
}

#[test]
fn malformed_remaining_package_fields_are_rejected_without_rewriting_the_source() {
    let package = Package::new();
    let project = fixture();
    write_package(&package.0, &project).unwrap();
    let track = &project.tracks()[0];
    let paths = [
        package.0.join("project.json"),
        package.0.join("tracks").join(format!("{}.json", track.id)),
        package
            .0
            .join("contents")
            .join(format!("{}.json", track.clips[0].content)),
    ];
    let originals: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let cases = [
        (0, "/meter", "unknown", json!(1)),
        (0, "/tempo/0", "unknown", json!(1)),
        (0, "/meter", "denominator", json!(0)),
        (0, "/tempo/0", "denominator", json!(0)),
        (1, "/clips/0", "unknown", json!(1)),
        (1, "/clips/0", "content", json!("not an id")),
        (1, "/clips/0", "lane", json!("other")),
        (1, "/clips/0", "kind", json!("other")),
        (1, "/clips/0", "gain", json!(-1)),
        (1, "/clips/0", "offset", json!(1.5)),
        (1, "/clips/0", "fade_out", json!(-1)),
        (2, "", "unknown", json!(1)),
        (2, "/audio", "unknown", json!(1)),
        (2, "/audio", "channels", json!(3)),
        (2, "", "takes", json!(["not an id"])),
    ];
    for (index, pointer, key, value) in cases {
        let mut file: Value = serde_json::from_slice(&originals[index]).unwrap();
        file.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), value);
        let bytes = serde_json::to_vec(&file).unwrap();
        std::fs::write(&paths[index], &bytes).unwrap();
        assert!(
            read_package(&package.0).is_err(),
            "accepted {pointer}/{key}"
        );
        for (other, path) in paths.iter().enumerate() {
            assert_eq!(
                std::fs::read(path).unwrap(),
                if index == other {
                    &bytes
                } else {
                    &originals[other]
                }
                .as_slice()
            );
        }
        std::fs::write(&paths[index], &originals[index]).unwrap();
    }
    for (index, pointer, keys) in [
        (0, "", vec!["meter", "tempo", "tracks"]),
        (
            1,
            "/clips/0",
            vec![
                "content", "lane", "start", "length", "offset", "kind", "gain", "fade_in",
                "fade_out",
            ],
        ),
        (2, "", vec!["takes", "notes", "expressions"]),
    ] {
        for key in keys {
            let mut file: Value = serde_json::from_slice(&originals[index]).unwrap();
            file.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            std::fs::write(&paths[index], serde_json::to_vec(&file).unwrap()).unwrap();
            assert!(
                read_package(&package.0).is_err(),
                "accepted missing {pointer}/{key}"
            );
            std::fs::write(&paths[index], &originals[index]).unwrap();
        }
    }
}
