use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_actions::Command;
use echode_history::{
    DocumentHost, PackageError, PackageKind, PackageValidationError, read_package, write_package,
};
use echode_model::{GAIN_MAX, Id, Project};
use serde_json::{Value, json};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "echode-route-package-{name}-{}-{n}.echode",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }

    fn track_file(&self, id: Id) -> PathBuf {
        self.0.join("tracks").join(format!("{id}.json"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn route_package_roundtrips_canonical_json_and_rejects_malformed_routes() {
    let dir = Scratch::new("canonical");
    let (project, kick) = Project::new().add_track("kick");
    let (project, bass) = project.add_track("bass");
    let (project, reverb) = project.add_track("reverb");
    let comp = Id::generate();
    let project = project
        .add_clap_device(bass, comp, "org.example.comp")
        .unwrap();
    let send = Id::generate();
    let project = project.add_send_with_id(send, kick, reverb, 0.5).unwrap();
    let sidechain = Id::generate();
    let project = project
        .add_sidechain_with_id(sidechain, kick, comp, 0.75)
        .unwrap();

    write_package(&dir.0, &project).unwrap();
    assert_eq!(read_package(&dir.0).unwrap(), project);

    let kick_json: Value =
        serde_json::from_str(&fs::read_to_string(dir.track_file(kick)).unwrap()).unwrap();
    assert_eq!(kick_json["schema"], 7);
    assert_eq!(
        kick_json["sends"],
        json!([{
            "id": send.to_string(),
            "target": reverb.to_string(),
            "gain": 0.5,
            "tap": "post_fx_pre_fader",
        }])
    );

    let bass_json: Value =
        serde_json::from_str(&fs::read_to_string(dir.track_file(bass)).unwrap()).unwrap();
    assert_eq!(
        bass_json["devices"][0]["sidechains"],
        json!([{
            "id": sidechain.to_string(),
            "source": kick.to_string(),
            "gain": 0.75,
            "tap": "post_fx_pre_fader",
        }])
    );

    // Reject a cycle introduced on disk without mutating the source file.
    let reverb_path = dir.track_file(reverb);
    let original_reverb = fs::read(&reverb_path).unwrap();
    let mut cycle_reverb: Value = serde_json::from_slice(&original_reverb).unwrap();
    cycle_reverb["sends"] = json!([{
        "id": Id::generate().to_string(),
        "target": kick.to_string(),
        "gain": 1.0,
        "tap": "post_fx_pre_fader",
    }]);
    let malformed = serde_json::to_vec(&cycle_reverb).unwrap();
    fs::write(&reverb_path, &malformed).unwrap();
    let error = read_package(&dir.0).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Validation(PackageValidationError::Model(
            echode_model::ModelError::Cycle
        ))
    ));
    assert_eq!(fs::read(&reverb_path).unwrap(), malformed);
    fs::write(&reverb_path, &original_reverb).unwrap();

    // Reject out-of-range gain, unknown tap, and sidechain on a mixer device.
    let bass_path = dir.track_file(bass);
    let original_bass = fs::read(&bass_path).unwrap();
    let mut bad_gain: Value = serde_json::from_slice(&original_bass).unwrap();
    bad_gain["devices"][0]["sidechains"][0]["gain"] = json!(GAIN_MAX + 1.0);
    let mut bad_tap: Value = serde_json::from_slice(&original_bass).unwrap();
    bad_tap["devices"][0]["sidechains"][0]["tap"] = json!("post_fader");
    let mut bad_mixer: Value = serde_json::from_slice(&original_bass).unwrap();
    bad_mixer["devices"][1]["sidechains"] = json!([{
        "id": Id::generate().to_string(),
        "source": kick.to_string(),
        "gain": 1.0,
        "tap": "post_fx_pre_fader",
    }]);
    for bad in [bad_gain, bad_tap, bad_mixer] {
        fs::write(&bass_path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let error = read_package(&dir.0).unwrap_err();
        assert!(
            matches!(
                error,
                PackageError::Validation(
                    PackageValidationError::Json {
                        kind: PackageKind::Track,
                        ..
                    } | PackageValidationError::Model(_)
                )
            ),
            "{error:?}"
        );
    }
    fs::write(&bass_path, &original_bass).unwrap();
    assert_eq!(read_package(&dir.0).unwrap(), project);
}

#[test]
fn route_edits_replay_from_journal_and_undo_across_device_deletion() {
    let dir = Scratch::new("journal");
    write_package(&dir.0, &Project::new()).unwrap();
    let host = DocumentHost::open(&dir.0).unwrap();
    let kick = match host
        .perform(Command::AddTrack {
            name: "kick".into(),
        })
        .unwrap()
    {
        echode_actions::Action::AddTrack { id, .. } => id,
        other => panic!("unexpected action: {other:?}"),
    };
    let bass = match host
        .perform(Command::AddTrack {
            name: "bass".into(),
        })
        .unwrap()
    {
        echode_actions::Action::AddTrack { id, .. } => id,
        other => panic!("unexpected action: {other:?}"),
    };
    let bus = match host
        .perform(Command::AddTrack { name: "bus".into() })
        .unwrap()
    {
        echode_actions::Action::AddTrack { id, .. } => id,
        other => panic!("unexpected action: {other:?}"),
    };
    let comp = match host
        .perform(Command::AddDevice {
            track: bass,
            plugin: "org.example.comp".into(),
        })
        .unwrap()
    {
        echode_actions::Action::AddDevice { device, .. } => device,
        other => panic!("unexpected action: {other:?}"),
    };

    let send = match host
        .perform(Command::AddSend {
            source: kick,
            target: bus,
            gain: None,
        })
        .unwrap()
    {
        echode_actions::Action::AddSend { id, .. } => id,
        other => panic!("unexpected action: {other:?}"),
    };
    host.perform(Command::SetSendGain { send, gain: 0.25 })
        .unwrap();

    let sidechain = match host
        .perform(Command::AddSidechain {
            source: kick,
            device: comp,
            gain: Some(0.5),
        })
        .unwrap()
    {
        echode_actions::Action::AddSidechain { id, .. } => id,
        other => panic!("unexpected action: {other:?}"),
    };
    host.perform(Command::SetSidechainGain {
        sidechain,
        gain: 0.875,
    })
    .unwrap();

    let with_routes = (*host.snapshot()).clone();
    assert_eq!(with_routes.send(send).unwrap().1.gain, 0.25);
    assert_eq!(with_routes.sidechain(sidechain).unwrap().2.gain, 0.875);

    // Removing the CLAP device removes its sidechains, and Undo restores both.
    host.perform(Command::RemoveDevice { device: comp })
        .unwrap();
    assert!(host.snapshot().sidechain(sidechain).is_none());
    host.undo().unwrap();
    assert_eq!(*host.snapshot(), with_routes);

    // Reopen from package + journal and verify identical state.
    drop(host);
    let reopened = DocumentHost::open(&dir.0).unwrap();
    assert_eq!(*reopened.snapshot(), with_routes);
}
