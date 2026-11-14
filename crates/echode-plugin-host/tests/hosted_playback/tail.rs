use super::*;

#[test]
fn changed_tails_replan_the_longest_folder_path_for_every_block_size_under_rtsan() {
    for block in [1, 7, 64, 512] {
        let (project, folder) = Project::new().add_track("folder");
        let (project, child) = project.add_child(folder, "child").unwrap();
        let devices = [Id::generate(), Id::generate()];
        let project = project
            .add_clap_device(folder, devices[0], "dev.echode.test-gain")
            .unwrap()
            .add_clap_device(child, devices[1], "dev.echode.test-gain")
            .unwrap();
        let routes = [PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: Some("tail-changed".into()),
            state: None,
        }];
        let (mut control, mut audio) =
            playback_with_plugins(&project, 48_000, block, &host_bin(), &routes).unwrap();
        assert_eq!(control.tail_samples(), 0);
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        for (state, expected) in [(64, 2048), (0, 0), (1, u32::MAX), (64, 2048)] {
            for device in devices {
                control
                    .restore_device_state(device.raw(), &[state])
                    .unwrap();
            }
            let changes = control.compensation_changes();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    Instant::now() + Duration::from_secs(5),
                    true,
                );
                control.service();
                assert!(control.take_preparation_error().is_none());
                if control.tail_samples() == expected && control.latency_settled() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "tail did not settle at {expected}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(control.compensation_changes() > changes);
            assert_eq!(control.latency_samples(), 0);
            assert!(!control.status().failed);
        }
    }
}
