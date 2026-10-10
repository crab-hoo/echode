use echode_engine::{PlaybackCommand, playback_pair};
use echode_model::{NoteDraft, Project, Tick};

fn score() -> Project {
    let (project, track) = Project::new().add_track("pad");
    project
        .insert_note(
            track,
            NoteDraft {
                pitch: 64,
                velocity: 100,
                start: Tick::new(0),
                length: Tick::quarters(2).unwrap(),
            },
        )
        .unwrap()
        .0
}

#[test]
fn pause_holds_the_position_stop_returns_to_zero_and_seek_chases_a_note() {
    let project = score();
    // Scratch lengths belong to this probe, never to a product track cap.
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
    assert!(left.iter().all(|sample| *sample == 0.0));
    control.command(PlaybackCommand::Play).unwrap();
    audio.render(&mut left, &mut right);
    assert_eq!(control.status().sample, 64);
    assert!(left.iter().any(|sample| *sample != 0.0));
    control.command(PlaybackCommand::Pause).unwrap();
    audio.render(&mut left, &mut right);
    assert_eq!(control.status().sample, 64);
    assert!(!control.status().playing);
    assert!(left.iter().all(|sample| *sample == 0.0));
    control.command(PlaybackCommand::Locate(24000)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    audio.render(&mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    assert_eq!(control.status().sample, 24064);
    control.command(PlaybackCommand::Stop).unwrap();
    audio.render(&mut left, &mut right);
    assert_eq!(control.status().sample, 0);
    assert_eq!(control.status().tick, 0);
    assert!(left.iter().all(|sample| *sample == 0.0));
}

#[test]
fn an_unchanged_live_voice_keeps_its_envelope_and_phase_across_an_edit() {
    let project = score();
    let pad = project.tracks()[0].id;
    let (changed, folder) = project.add_track("empty folder");
    // Reorder the pad through the model's folder operation. Playback keys keep
    // their identity even when their prepared track indices change.
    let changed = changed.set_parent(pad, Some(folder)).unwrap();
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    let (mut reference_control, mut reference) = playback_pair(&project, 48000, 64).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    reference_control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right, mut expected_left, mut expected_right) =
        ([0.0; 64], [0.0; 64], [0.0; 64], [0.0; 64]);
    for block in 0..1100 {
        if block == 20 || block == 800 {
            control.replace(&changed).unwrap();
        }
        audio.render(&mut left, &mut right);
        reference.render(&mut expected_left, &mut expected_right);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
        control.service();
    }
}

#[test]
fn deleting_a_sounding_note_silences_it_in_the_next_plan() {
    let project = score();
    let (mut control, mut audio) = playback_pair(&project, 48000, 64).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
    assert!(left.iter().any(|sample| *sample != 0.0));
    control.replace(&Project::new()).unwrap();
    audio.render(&mut left, &mut right);
    assert!(left.iter().all(|sample| *sample == 0.0));
    assert_eq!(control.status().sample, 128);
}
