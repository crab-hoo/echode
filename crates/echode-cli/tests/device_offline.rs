#[path = "support/output.rs"]
mod output;
use output::output_path;

use echode_actions::{Action, Command as Edit};
use echode_cli::write_wav;
use echode_history::{DocumentHost, import_audio, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

#[test]
fn an_offline_copy_does_not_supply_state_to_an_online_copy_of_the_same_plugin() {
    let directory = std::env::temp_dir().join(format!("echode-offline-routes-{}", Id::generate()));
    std::fs::create_dir_all(&directory).unwrap();
    let (project, track) = Project::new().add_track("copies");
    let offline = Id::generate();
    let project = project
        .add_clap_device(track, offline, "dev.echode.sine-pad")
        .unwrap()
        .set_device_state(offline, Some("f".repeat(64)))
        .unwrap()
        .set_device_offline(offline, true)
        .unwrap()
        .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
        .unwrap();
    // The offline copy's missing state object is never named to a helper.
    let routes = echode_audio_io::routes_with_saved_state(&project, &directory);
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].clap_id, "dev.echode.sine-pad");
    assert!(routes[0].state.is_none());
    std::fs::remove_dir_all(directory).unwrap();
}
use std::process::Command;

#[test]
fn contract_offline_missing_plugin_exports_dry_audio_and_keeps_its_state() {
    let directory = std::env::temp_dir().join(format!("echode-offline-export-{}", Id::generate()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("source.wav");
    // This short mono source bounds this CLI fixture only, not project length.
    write_wav(&source, 48_000, &vec![0.25; 256]).unwrap();
    let stored = import_audio(&directory, &source).unwrap();
    let (project, track) = Project::new().add_track("source");
    let project = project
        .place_audio(
            track,
            Id::generate(),
            Id::generate(),
            Tick::new(0),
            AudioRef::try_new(
                stored.hash,
                stored.sample_rate,
                stored.channels,
                stored.frames,
            )
            .unwrap(),
        )
        .unwrap();
    write_package(&directory, &project).unwrap();
    let baseline_wav = output_path("base.wav");
    let offline_wav = output_path("offline.wav");
    let render = |wav: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_echode"))
            .args([
                "render",
                directory.to_str().unwrap(),
                wav.to_str().unwrap(),
                "--frames",
                "128",
            ])
            .output()
            .unwrap()
    };
    let baseline = render(&baseline_wav);
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let host = DocumentHost::open(&directory).unwrap();
    let Action::AddDevice { device, .. } = host
        .perform(Edit::AddDevice {
            track,
            plugin: "missing.offline.fixture".into(),
        })
        .unwrap()
    else {
        panic!("add device");
    };
    host.commit_plugin_state(device, b"recorded settings")
        .unwrap();
    host.perform(Edit::OfflineDevice {
        device,
        offline: true,
    })
    .unwrap();
    let hash = host.snapshot().tracks()[0].devices[0].state.clone();
    drop(host);
    let offline = render(&offline_wav);
    assert!(
        offline.status.success(),
        "{}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert_eq!(
        std::fs::read(baseline_wav).unwrap(),
        std::fs::read(offline_wav).unwrap()
    );
    let host = DocumentHost::open(&directory).unwrap();
    assert_eq!(host.snapshot().tracks()[0].devices[0].state, hash);
    host.perform(Edit::OfflineDevice {
        device,
        offline: false,
    })
    .unwrap();
    drop(host);
    assert!(
        !render(&output_path("online.wav")).status.success(),
        "an online missing plugin must still fail export"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
