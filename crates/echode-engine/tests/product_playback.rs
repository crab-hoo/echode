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
fn notes_without_an_instrument_do_not_create_an_implicit_synth() {
    let project = score();
    // These buffers cover this device-free regression only.
    let (mut control, mut audio) = playback_pair(&project, 48_000, 128).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    audio.render(&mut left, &mut right);
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
}
