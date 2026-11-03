//! Open items from docs/reviews/2026-11-03-full-code-audit.md. Each test fails
//! until its item is fixed; `make known-bugs` runs them. Remove the `#[ignore]`
//! with the fix. The release measurements are `make spike-scale`.

use std::sync::Arc;

use echode_engine::{LoadedClip, PlaybackCommand, PlaybackOptions, playback};
use echode_model::{DeviceKind, Id, NoteDraft, ParamKey, Project, TICKS_PER_QUARTER, Tick};

const RATE: u32 = 48_000;
const FRAMES: usize = 64;

/// `notes` eighth notes, one per beat, on one track.
fn evenly_spaced(notes: usize) -> Project {
    let (mut project, track) = Project::new().add_track("piano");
    for index in 0..notes {
        project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(index as i64 * TICKS_PER_QUARTER),
                    length: Tick::new(TICKS_PER_QUARTER / 2),
                },
            )
            .unwrap()
            .0;
    }
    project
}

fn reserved_events(project: &Project) -> usize {
    let (control, _audio) = playback(project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.reserved_events()
}

/// FCA-03: one block of 64 frames holds at most one note's edges here, however
/// long the score is. Slots sized by every note on the track also size each
/// plugin's locked shared memory and remap it whenever a note is added.
#[test]
fn audit_fca03_event_slots_follow_one_block_not_the_whole_track() {
    let short = reserved_events(&evenly_spaced(64));
    let long = reserved_events(&evenly_spaced(2_048));
    assert_eq!(
        long, short,
        "the same density reserved {short} slots for 64 notes and {long} for 2,048"
    );
}

#[test]
fn replacing_a_dense_score_with_an_empty_plan_keeps_release_space() {
    let (mut project, track) = Project::new().add_track("chord");
    for _ in 0..128 {
        // This fixture's chord size, not a product limit.
        project = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::quarters(1).unwrap(),
                },
            )
            .unwrap()
            .0;
    }
    let (mut control, mut audio) =
        playback(&project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; FRAMES];
    let mut right = left.clone();
    audio.render(&mut left, &mut right);
    control.replace(&Project::new()).unwrap();
    audio.render(&mut left, &mut right);
    assert!(
        !control.status().failed,
        "removed tracks' note-offs exceeded the empty plan's bank"
    );
}

fn mixer_parameter(project: &Project, track: Id, key: ParamKey) -> Id {
    let track = project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)
        .unwrap();
    let mixer = track
        .devices
        .iter()
        .find(|device| device.kind == DeviceKind::Mixer)
        .unwrap();
    mixer
        .parameters
        .iter()
        .find(|parameter| parameter.key == key)
        .unwrap()
        .id
}

/// Loudest sample of the first blocks, with a constant clip on `child`.
fn loudest(project: &Project, child: Id) -> f32 {
    let (mut control, mut audio) =
        playback(project, RATE, FRAMES, PlaybackOptions::default()).unwrap();
    let track = child.raw();
    control.set_clip_loader(Arc::new(move |_project, _rate| {
        Ok(vec![LoadedClip {
            track,
            start_sample: 0,
            samples: Arc::from(vec![0.5f32; RATE as usize]),
            right: None,
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0f32; FRAMES];
    let mut right = vec![0.0f32; FRAMES];
    let mut peak = 0.0f32;
    for _ in 0..4 {
        audio.render(&mut left, &mut right);
        control.service();
        peak = left
            .iter()
            .chain(&right)
            .fold(peak, |peak, sample| peak.max(sample.abs()));
    }
    peak
}

/// FCA-06: the engine has no track hierarchy. A folder's mute does not reach
/// its children, and soloing the folder silences them. Until folders become
/// buses (P2-03), ADR the chosen meaning; if folder controls are disabled
/// instead, replace this with a test of that contract.
#[test]
#[ignore = "FCA-06: folder mute and solo do not reach the children"]
fn audit_fca06_a_folder_mute_and_solo_reach_its_children() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, child) = project.add_child(folder, "child").unwrap();
    assert!(
        loudest(&project, child) > 0.1,
        "the child's clip is audible"
    );

    let muted = project
        .set_parameter(mixer_parameter(&project, folder, ParamKey::Mute), 1.0)
        .unwrap();
    assert_eq!(
        loudest(&muted, child),
        0.0,
        "muting the folder left its child audible"
    );

    let soloed = project
        .set_parameter(mixer_parameter(&project, folder, ParamKey::Solo), 1.0)
        .unwrap();
    assert!(
        loudest(&soloed, child) > 0.1,
        "soloing the folder silenced its child"
    );
}
