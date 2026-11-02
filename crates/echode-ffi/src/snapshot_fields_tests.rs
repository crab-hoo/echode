//! Public snapshot ABI expectations independent of scalar field registries.
use super::*;
use echode_history::{read_package, write_package};
use echode_model::{AudioRef, Id, Project, Tick};
use std::sync::Arc;

#[test]
fn package_scalars_reach_retained_snapshot_controls_without_a_device_or_helper() {
    let root = std::env::temp_dir().join(format!("echode-snapshot-fields-{}", Id::generate()));
    let (project, folder) = Project::new().add_track("folder");
    let (project, track) = project.add_child(folder, "日本語 🎹").unwrap();
    let device = Id::generate();
    let clip = Id::generate();
    let project = project
        .add_clap_device(track, device, "org.example.fixture")
        .unwrap()
        .set_device_bypassed(device, true)
        .unwrap()
        .set_device_offline(device, true)
        .unwrap()
        .set_parameter_pinned(device, 17, true)
        .unwrap()
        .set_parameter_pinned(device, 0, true)
        .unwrap()
        .place_audio_with_fades(
            track,
            clip,
            Id::generate(),
            Tick::new(123),
            AudioRef::try_new("ab".repeat(32), 48_000, 2, 96_000).unwrap(),
            17,
            31,
        )
        .unwrap()
        .set_clip_gain(clip, f32::from_bits(1))
        .unwrap();
    write_package(&root, &project).unwrap();
    let snapshot = Snapshot::from_project(Arc::new(read_package(&root).unwrap()), None);
    let retained = snapshot.retain();
    drop(snapshot);
    unsafe {
        let mut length = usize::MAX;
        let name = echode_snapshot_track_name(&retained, 1, &mut length);
        assert_eq!(
            std::slice::from_raw_parts(name, length),
            "日本語 🎹".as_bytes()
        );
        assert_eq!(echode_snapshot_device_bypassed(&retained, 1, 0), 1);
        assert_eq!(echode_snapshot_device_offline(&retained, 1, 0), 1);
        assert_eq!(echode_snapshot_device_bypassed(&retained, 1, 1), 0);
        assert_eq!(echode_snapshot_device_offline(&retained, 1, 1), 0);
        assert_eq!(echode_snapshot_device_offline(&retained, 2, 0), -1);
        assert_eq!(echode_snapshot_device_bypassed(&retained, 1, 2), -1);
        let plugin = echode_snapshot_device_plugin(&retained, 1, 0, &mut length);
        assert_eq!(
            std::slice::from_raw_parts(plugin, length),
            b"org.example.fixture"
        );
        let pins = echode_snapshot_device_pins(&retained, 1, 0, &mut length);
        assert_eq!(std::slice::from_raw_parts(pins, length), &[0, 17]);
        assert!(echode_snapshot_device_plugin(&retained, 1, 1, &mut length).is_null());
        assert_eq!(length, 0);
        assert!(echode_snapshot_device_pins(&retained, 1, 2, &mut length).is_null());
        assert_eq!(length, 0);
        let mut controls = ClipControls::default();
        let bytes = uuid::Uuid::from_u128(clip.raw()).into_bytes();
        assert_eq!(
            echode_snapshot_clip_controls(&retained, bytes.as_ptr(), &mut controls),
            ECHODE_OK
        );
        assert_eq!(
            controls.track_id,
            uuid::Uuid::from_u128(track.raw()).into_bytes()
        );
        assert_eq!(controls.clip_id, bytes);
        assert_eq!(controls.start, 123);
        assert_eq!(controls.length, project.tracks()[1].clips[0].length.get());
        assert_eq!(controls.fade_in, 17);
        assert_eq!(controls.fade_out, 31);
        assert_eq!(controls.gain.to_bits(), 1);
        assert_eq!(controls.audio, 1);
        assert_eq!(controls.fade_in_ticks, 0);
        assert_eq!(controls.fade_out_ticks, 0);
    }
    std::fs::remove_dir_all(root).unwrap();
}
