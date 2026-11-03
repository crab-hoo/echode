//! Each track's notes reach that track's helper and no other (ADR 0236).
//!
//! The callback groups a block's events by track once. A grouping that hands
//! one track's notes to every track would still pass a test whose tracks all
//! play the same notes, so each track here holds a different number.

use super::*;

/// Note counts, one per track, in project order. All notes start together
/// and outlast the rendered blocks.
const CHORDS: [usize; 3] = [1, 2, 3];

fn chord_tracks() -> (Project, Vec<Id>) {
    let mut project = Project::new();
    let mut tracks = Vec::new();
    for (index, notes) in CHORDS.iter().enumerate() {
        let (next, track) = project.add_track(format!("chord-{index}"));
        project = next;
        for offset in 0..*notes {
            project = project
                .insert_note(
                    track,
                    NoteDraft {
                        pitch: 60 + offset as u8,
                        velocity: 100,
                        start: Tick::new(0),
                        length: Tick::quarters(2).unwrap(),
                    },
                )
                .unwrap()
                .0;
        }
        tracks.push(track);
    }
    (project, tracks)
}

fn with_counters(mut project: Project, tracks: &[Id], first_device: u128) -> Project {
    for (index, track) in tracks.iter().enumerate() {
        project = project
            .add_clap_device(
                *track,
                Id::from_raw(first_device + index as u128),
                "dev.echode.test-gain",
            )
            .unwrap();
    }
    project
}

fn live(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_live_gated_until(left, right, Instant::now() + Duration::from_secs(5), true);
}

/// Post-fader peaks, in the order of `tracks`.
fn peaks(control: &echode_engine::PlaybackControl, tracks: &[Id]) -> Vec<f32> {
    let meters = control.meters();
    tracks
        .iter()
        .map(|track| {
            meters
                .iter()
                .find(|meter| meter.track == track.raw())
                .expect("track meter")
                .peak()
        })
        .collect()
}

#[test]
fn contract_each_track_hears_only_its_own_notes_offs_and_chase_under_rtsan() {
    let route = count_route(&plugin_dylib());
    let frames = 256usize;
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];

    // Timeline note-ons in one block.
    let (project, tracks) = chord_tracks();
    let hosted = with_counters(project.clone(), &tracks, 700);
    let (mut control, mut audio) = playback_with_plugins(
        &hosted,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    for (heard, notes) in peaks(&control, &tracks).into_iter().zip(CHORDS) {
        assert_level(heard, notes as f32);
    }
    // A stop sends each held note's off to its own helper. The counter keeps
    // its totals through the reset, so a later block past the notes shows them.
    control.command(PlaybackCommand::Stop).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    control.command(PlaybackCommand::Locate(480_000)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    for (heard, notes) in peaks(&control, &tracks).into_iter().zip(CHORDS) {
        assert_level(heard, notes as f32 * 17.0);
    }
    assert!(!control.status().failed);

    // A live locate resets every helper in a discarded block that carries the
    // held notes' offs. Each helper must get its own track's offs there too.
    let (mut control, mut audio) = playback_with_plugins(
        &with_counters(project.clone(), &tracks, 900),
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    live(&mut audio, &mut left, &mut right);
    control.command(PlaybackCommand::Locate(480_000)).unwrap();
    live(&mut audio, &mut left, &mut right);
    // A live callback polls the cleanup once and never waits. These blocks
    // run faster than real time, so give the helpers wall time to finish.
    let started = Instant::now();
    while audio.cleanup_pending() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the locate's cleanup did not finish"
        );
        live(&mut audio, &mut left, &mut right);
    }
    live(&mut audio, &mut left, &mut right);
    for (heard, notes) in peaks(&control, &tracks).into_iter().zip(CHORDS) {
        assert_level(heard, notes as f32 * 17.0);
    }
    assert!(!control.status().failed);

    // The notes start with no helper. Helpers added while they sound are
    // chased with only their own track's held notes.
    let (mut control, mut audio) = playback_with_plugins(
        &project,
        48_000,
        frames,
        &host_bin(),
        std::slice::from_ref(&route),
    )
    .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    render_content(&mut audio, &mut left, &mut right);
    control
        .replace(&with_counters(project, &tracks, 800))
        .unwrap();
    render_content(&mut audio, &mut left, &mut right);
    for (heard, notes) in peaks(&control, &tracks).into_iter().zip(CHORDS) {
        assert_level(heard, notes as f32);
    }
    assert!(!control.status().failed);
}
