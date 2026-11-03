use super::*;

fn add_note(project: Project, track: Id, quarter: i64) -> Project {
    project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::quarters(quarter).unwrap(),
                length: Tick::quarters(1).unwrap(),
            },
        )
        .unwrap()
        .0
}

#[test]
fn shared_event_banks_follow_density_and_only_grow_under_rtsan() {
    let path = plugin_dylib();
    let (base, track) = Project::new().add_track("density");
    let base = base
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let base = add_note(add_note(base, track, 0), track, 2);
    let mut long = base.clone();
    for index in 2..2048 {
        long = add_note(long, track, index * 2);
    }
    let frames = 64; // This regression's block length, not a product limit.
    let (mut control, mut audio) =
        playback_with_plugins(&base, 48_000, frames, &host_bin(), &[count_route(&path)]).unwrap();
    let original = control.plugin_event_caps();
    let (long_control, long_audio) =
        playback_with_plugins(&long, 48_000, frames, &host_bin(), &[count_route(&path)]).unwrap();
    assert_eq!(
        long_control.plugin_event_caps(),
        original,
        "score length changed the shared bank"
    );
    assert!(
        original
            .iter()
            .all(|cap| cap.is_power_of_two() && *cap <= 8)
    );
    drop((long_control, long_audio));
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    render_content(&mut audio, &mut left, &mut right);
    assert_level(left[0], 1.0);
    control.replace(&long).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    control.service();
    assert_eq!(control.plugin_event_caps(), original);
    assert_level(left[0], 1.0);

    let mut dense = base.clone();
    for _ in 0..64 {
        // Fixture polyphony only; exercise growth and mass release.
        dense = add_note(dense, track, 0);
    }
    control.replace(&dense).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    control.service();
    assert_level(left[0], 65.0);
    let grown = control.plugin_event_caps();
    assert!(grown[0] > original[0] && grown[0].is_power_of_two());
    control.replace(&base).unwrap();
    for _ in 0..4 {
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert!(
            !control.status().failed,
            "old voices did not fit the new plan's releases"
        );
        assert!(
            control.plugin_failure().is_none(),
            "{:?}",
            control.plugin_failure()
        );
        assert_level(left[0], 65.0 + 64.0 * 16.0);
    }
    assert_eq!(
        control.plugin_event_caps(),
        grown,
        "a smaller demand shrank the shared bank"
    );
    assert_eq!(control.status().plugin_missed_blocks, 0);
}
