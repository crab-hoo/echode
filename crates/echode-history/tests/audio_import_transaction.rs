use echode_actions::{Action, JournalEntry};
use echode_history::{DocumentHost, HostError, write_package};
use echode_model::{Id, Project};
use serde_json::json;

#[test]
fn contract_audio_import_transaction_adds_a_track_atomically_and_reopens() {
    let root = std::env::temp_dir().join(format!("echode-import-transaction-{}", Id::generate()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("tone.wav");
    std::fs::write(&source, wav()).unwrap();
    let directory = root.join("Song.echode");
    write_package(&directory, &Project::new()).unwrap();
    let host = DocumentHost::open(&directory).unwrap();
    let commands = || {
        vec![
            json!({"type": "track.add", "name": "audio"}),
            json!({"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 705_600, "path": source}),
        ]
    };
    let actions = host
        .apply(commands())
        .expect("audio.import must work in an atomic transaction");
    assert!(matches!(
        actions.as_slice(),
        [Action::AddTrack { .. }, Action::PlaceAudio { .. }]
    ));
    let imported = host.snapshot();
    imported.check_invariants().unwrap();
    assert_eq!(imported.tracks().len(), 1);
    // Imported audio keeps its recorded edges (ADR 0161).
    let placed = &imported.tracks()[0].clips[0];
    assert_eq!((placed.fade_in, placed.fade_out), (0, 0));
    host.undo().unwrap();
    assert!(host.snapshot().tracks().is_empty());
    host.redo().unwrap();
    assert_eq!(*host.snapshot(), *imported);
    // A later failing element must roll back the track and preserve redo.
    host.undo().unwrap();
    let mut failed = commands();
    failed.push(json!({"type": "track.add_child", "parent": Id::generate(), "name": "invalid"}));
    assert!(matches!(
        host.apply(failed),
        Err(HostError::Element {
            index: 2,
            parameters: false,
            ..
        })
    ));
    assert!(host.snapshot().tracks().is_empty());
    host.redo().unwrap();
    assert_eq!(*host.snapshot(), *imported);
    let mut missing = commands();
    missing[1]["path"] = json!(root.join("missing.wav"));
    assert!(matches!(
        host.apply(missing),
        Err(HostError::Element {
            index: 1,
            parameters: false,
            ..
        })
    ));
    assert_eq!(*host.snapshot(), *imported);
    host.undo().unwrap();
    std::fs::remove_file(&source).unwrap();
    // Redo and recovery use the stored PCM, not the original external path.
    host.redo().unwrap();
    assert_eq!(*host.snapshot(), *imported);
    drop(host);
    let reopened = DocumentHost::open(&directory).unwrap();
    assert_eq!(*reopened.snapshot(), *imported);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

fn wav() -> Vec<u8> {
    // This tiny mono fixture bounds only this import regression, not product audio.
    let samples: [i16; 4] = [0, 8192, -8192, 0];
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Journals written before ADR 0161 have no fade fields on `audio.place`.
/// They replay the 5 ms edges they were recorded with; new entries say zero.
#[test]
fn contract_a_journal_place_without_fades_replays_the_old_edges() {
    let (project, track) = Project::new().add_track("audio");
    let mut legacy = json!({
        "type": "audio.place", "track": track, "clip": Id::generate(), "content": Id::generate(),
        "start": 0, "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "sample_rate": 48_000, "channels": 1, "frames": 4_800,
    });
    let replay = |action: Action| {
        echode_actions::replay(
            &project,
            &[JournalEntry {
                actions: vec![action],
                changes: None,
            }],
        )
        .unwrap()
    };
    let replayed = replay(serde_json::from_value(legacy.clone()).unwrap());
    let clip = &replayed.tracks()[0].clips[0];
    assert_eq!(
        (clip.fade_in, clip.fade_out),
        (
            echode_model::DEFAULT_FADE_FLICKS,
            echode_model::DEFAULT_FADE_FLICKS
        )
    );
    legacy["fade_in"] = json!(0);
    legacy["fade_out"] = json!(0);
    let current: Action = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&current).unwrap(), legacy);
    let current = replay(current);
    let clip = &current.tracks()[0].clips[0];
    assert_eq!((clip.fade_in, clip.fade_out), (0, 0));
}
