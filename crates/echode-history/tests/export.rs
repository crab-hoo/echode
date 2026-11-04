//! Export copies recorded objects, not a mutable package or its whole store.

use std::fs;
use std::path::PathBuf;

use echode_history::{freeze_export, read_object, read_package, write_object, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-freeze-{}", Id::generate()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn frozen_package_copies_only_referenced_audio_and_plugin_state() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source.echode");
    let frozen = fixture.0.join("snapshot.echode");
    let audio_hash = write_object(&source, b"recorded audio bytes").unwrap();
    let state_hash = write_object(&source, b"recorded plugin state").unwrap();
    let unused = write_object(&source, b"unused older state").unwrap();
    let (project, track) = Project::new().add_track("captured");
    let device = Id::generate();
    let project = project
        .place_audio(
            track,
            Id::generate(),
            Id::generate(),
            Tick::new(0),
            AudioRef::try_new(audio_hash.clone(), 48_000, 1, 64).unwrap(),
        )
        .unwrap()
        .add_clap_device(track, device, "org.example.export-fixture")
        .unwrap()
        .set_device_state(device, Some(state_hash.clone()))
        .unwrap();
    write_package(&source, &project).unwrap();
    freeze_export(&source, &project, &frozen).unwrap();
    assert!(!frozen.join("media/objects").join(unused).exists());
    assert!(!frozen.join("journal.jsonl").exists());
    assert!(!frozen.join(".git").exists());
    assert_eq!(
        fs::read(source.join("project.json")).unwrap(),
        fs::read(frozen.join("project.json")).unwrap()
    );
    fs::remove_dir_all(source).unwrap();
    assert_eq!(
        read_object(&frozen, &audio_hash).unwrap(),
        b"recorded audio bytes"
    );
    assert_eq!(
        read_object(&frozen, &state_hash).unwrap(),
        b"recorded plugin state"
    );
    let recovered = read_package(&frozen).unwrap();
    recovered.check_invariants().unwrap();
    assert_eq!(recovered.tracks()[0].name, "captured");
    assert_eq!(
        recovered.contents()[0].audio.as_ref().unwrap().hash,
        audio_hash
    );
    assert_eq!(
        recovered.tracks()[0].devices[0].state.as_deref(),
        Some(state_hash.as_str())
    );
}

#[test]
fn existing_destinations_and_links_are_preserved_and_failed_copies_are_removed() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let existing = fixture.0.join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("sentinel"), b"keep").unwrap();
    let linked = fixture.0.join("link");
    std::os::unix::fs::symlink(&existing, &linked).unwrap();
    for destination in [&existing, &linked] {
        assert!(freeze_export(&source, &Project::new(), destination).is_err());
        assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&existing).unwrap().count(), 1);
    }
    let hash = write_object(&source, b"original state").unwrap();
    let (project, track) = Project::new().add_track("missing state");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "org.example.export-fixture")
        .unwrap()
        .set_device_state(device, Some(hash.clone()))
        .unwrap();
    let destination = fixture.0.join("failed");
    fs::write(source.join("media/objects").join(&hash), b"damaged").unwrap();
    let error = freeze_export(&source, &project, &destination).unwrap_err();
    assert!(error.to_string().contains("does not match its BLAKE3"));
    assert!(!destination.exists());
    fs::remove_file(source.join("media/objects").join(&hash)).unwrap();
    assert!(freeze_export(&source, &project, &destination).is_err());
    assert!(!destination.exists());
}
