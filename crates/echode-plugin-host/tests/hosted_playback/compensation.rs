//! Plugin delay compensation through real helpers (ADR 0339).
//!
//! The test plugin's `delay:<frames>` fault delays its main audio by the
//! latency it reports, so a path's arrival at master is measurable. Every
//! clip here is a one-frame impulse, so a misaligned path shows as a second
//! impulse rather than a level.
use super::*;

/// Only this fixture's sections; the engine has no length limit.
const SECTION: usize = 160;
const BLOCKS: [usize; 4] = [1, 7, 64, 512];

fn delay_route(clap_id: &str, frames: &str, instrument: bool) -> PluginRoute {
    PluginRoute {
        clap_id: clap_id.into(),
        path: plugin_dylib(),
        instrument,
        fault: Some(format!("delay:{frames}")),
        state: None,
    }
}

fn impulses(clips: Vec<(Id, i64, f32, f32)>) -> echode_engine::ClipLoader {
    Arc::new(move |_, _| {
        Ok(clips
            .iter()
            .map(|&(track, start_sample, left, right)| LoadedClip {
                track: track.raw(),
                start_sample,
                samples: Arc::from(vec![left]),
                right: Some(Arc::from(vec![right])),
                window: None,
                gain: 1.0,
                fade_in: 0,
                fade_out: 0,
            })
            .collect())
    })
}

/// Plays `frames` in blocks of `block` and returns both channels.
fn play(
    project: &Project,
    rate: u32,
    block: usize,
    routes: &[PluginRoute],
    clips: echode_engine::ClipLoader,
    frames: usize,
) -> (echode_engine::PlaybackControl, Vec<f32>, Vec<f32>) {
    let (mut control, mut audio) =
        playback_with_plugins(project, rate, block, &host_bin(), routes).unwrap();
    control.set_clip_loader(clips);
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut heard_left, mut heard_right) = (Vec::new(), Vec::new());
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    while heard_left.len() < frames {
        let count = block.min(frames - heard_left.len());
        render_content(&mut audio, &mut left[..count], &mut right[..count]);
        heard_left.extend_from_slice(&left[..count]);
        heard_right.extend_from_slice(&right[..count]);
        assert!(!control.status().failed);
        assert!(control.plugin_failure().is_none());
    }
    (control, heard_left, heard_right)
}

fn assert_impulse(heard: &[f32], at: usize, level: f32, context: &str) {
    for (frame, sample) in heard.iter().enumerate() {
        let expected = if frame == at { level } else { 0.0 };
        assert!(
            (sample - expected).abs() < 1.0e-6,
            "{context}: frame {frame} is {sample}, expected {expected}"
        );
    }
}

/// FCA-07: a track behind a delaying effect played that much later than
/// the others. Both impulses now meet at master, one latency late.
#[test]
fn sibling_tracks_meet_at_master_for_every_block_size_under_rtsan() {
    let (project, delayed) = Project::new().add_track("delayed");
    let (project, dry) = project.add_track("dry");
    let project = project
        .add_clap_device(delayed, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let routes = [delay_route("dev.echode.test-gain", "37", false)];
    for rate in [48_000, 96_000] {
        for block in BLOCKS {
            let clips = impulses(vec![(delayed, 20, 0.5, -0.25), (dry, 20, 0.125, 0.0625)]);
            let (control, left, right) = play(&project, rate, block, &routes, clips, SECTION);
            let context = format!("rate {rate}, block {block}");
            assert_impulse(&left, 57, 0.625, &context);
            assert_impulse(&right, 57, -0.1875, &context);
            assert_eq!(control.latency_samples(), 37);
            assert_eq!(control.status().latency_samples, 37);
        }
    }
}

/// A folder's own clip and first instrument wait for its slowest child, and
/// the folder's delaying effect is waited for at master (ADR 0277, ADR 0339).
#[test]
fn a_folder_aligns_its_clip_its_instrument_and_its_children_under_rtsan() {
    let (project, root) = Project::new().add_track("folder");
    let (project, slow) = project.add_child(root, "slow").unwrap();
    let (project, fast) = project.add_child(root, "fast").unwrap();
    let (project, outside) = project.add_track("outside");
    let note = NoteDraft {
        pitch: 60,
        velocity: 100,
        start: Tick::new(0),
        length: Tick::quarters(1).unwrap(),
    };
    let project = project
        .insert_note(root, note)
        .unwrap()
        .0
        .add_clap_device(root, Id::generate(), "dev.echode.test-instrument")
        .unwrap()
        .add_clap_device(root, Id::generate(), "dev.echode.test-gain")
        .unwrap()
        .add_clap_device(slow, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    // Instrument 16, the folder's effect 8, the slow child 40. The folder's
    // input meets at 40, so master is 48 late. Routes go out in track order.
    let routes = [
        delay_route("dev.echode.test-instrument", "16", true),
        delay_route("dev.echode.test-gain", "8", false),
        delay_route("dev.echode.test-gain", "40", false),
    ];
    for block in BLOCKS {
        let clips = impulses(vec![
            (root, 0, 0.5, 0.5),
            (slow, 0, 0.125, -0.125),
            (fast, 0, 0.0625, 0.25),
            (outside, 0, 1.0, 2.0),
        ]);
        let (control, left, right) = play(&project, 48_000, block, &routes, clips, SECTION);
        for (frame, (left, right)) in left.iter().zip(&right).enumerate() {
            // The instrument's 0.25 gate starts with its note, at the same frame.
            let gate = if frame >= 48 { 0.25 } else { 0.0 };
            let (impulse_left, impulse_right) = if frame == 48 {
                (0.5 + 0.125 + 0.0625 + 1.0, 0.5 - 0.125 + 0.25 + 2.0)
            } else {
                (0.0, 0.0)
            };
            assert!(
                (left - (gate + impulse_left)).abs() < 1.0e-6
                    && (right - (gate + impulse_right)).abs() < 1.0e-6,
                "block {block}, frame {frame}: ({left}, {right})"
            );
        }
        assert_eq!(control.latency_samples(), 48);
    }
}

/// A bypassed device keeps its latency, so bypassing it moves no track.
#[test]
fn bypassing_a_delaying_effect_keeps_every_track_in_place_under_rtsan() {
    let device = Id::generate();
    let (project, delayed) = Project::new().add_track("delayed");
    let (project, dry) = project.add_track("dry");
    let active = project
        .add_clap_device(delayed, device, "dev.echode.test-gain")
        .unwrap();
    let bypassed = active.set_device_bypassed(device, true).unwrap();
    let routes = [delay_route("dev.echode.test-gain", "32", false)];
    for project in [&active, &bypassed] {
        for block in [7, 64] {
            let clips = impulses(vec![(delayed, 10, 0.5, 0.5), (dry, 10, 0.25, 0.25)]);
            let (control, left, _) = play(project, 48_000, block, &routes, clips, 96);
            assert_impulse(&left, 42, 0.75, &format!("block {block}"));
            assert_eq!(control.latency_samples(), 32);
        }
    }
}

/// A helper that restarts with another latency is followed by a new plan
/// once control services it. After a locate both paths meet at the new value.
#[test]
fn a_restart_with_another_latency_moves_the_compensation() {
    let (project, delayed) = Project::new().add_track("delayed");
    let (project, dry) = project.add_track("dry");
    let project = project
        .add_clap_device(delayed, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let routes = [delay_route("dev.echode.test-gain", "16:48", false)];
    let block = 64; // This test's scratch size, not an engine limit.
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, block, &host_bin(), &routes).unwrap();
    control.set_clip_loader(impulses(vec![(delayed, 0, 0.5, 0.5), (dry, 0, 0.25, 0.25)]));
    control.replace(&project).unwrap();
    assert_eq!(control.latency_samples(), 16);
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while control.status().latency_samples != 48 {
        assert!(
            Instant::now() < deadline,
            "the new latency was not followed"
        );
        render_content(&mut audio, &mut left, &mut right);
        control.service();
        assert!(control.take_preparation_error().is_none());
    }
    assert_eq!(control.latency_samples(), 48);
    control.command(PlaybackCommand::Locate(0)).unwrap();
    let mut heard = Vec::new();
    while heard.len() < 3 * block {
        render_content(&mut audio, &mut left, &mut right);
        heard.extend_from_slice(&left);
    }
    assert_impulse(&heard, 48, 0.75, "after the restart");
}

/// The metronome is heard with the music it counts, one latency late.
#[test]
fn the_metronome_waits_with_the_music_under_rtsan() {
    let (project, track) = Project::new().add_track("delayed");
    let delayed = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let routes = [delay_route("dev.echode.test-gain", "37", false)];
    let block = 64; // This test's scratch size, not an engine limit.
    let clicks = |project: &Project| {
        let (mut control, mut audio) =
            playback_with_plugins(project, 48_000, block, &host_bin(), &routes).unwrap();
        control
            .command(PlaybackCommand::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        let mut heard = Vec::new();
        for _ in 0..4 {
            render_content(&mut audio, &mut left, &mut right);
            heard.extend_from_slice(&left);
        }
        heard
    };
    let dry = clicks(&project);
    let late = clicks(&delayed);
    assert!(dry.iter().any(|sample| *sample != 0.0));
    assert!(late[..37].iter().all(|sample| *sample == 0.0));
    assert_eq!(late[37..], dry[..dry.len() - 37]);
}
