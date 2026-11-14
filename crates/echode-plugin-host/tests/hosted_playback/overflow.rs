//! Regression coverage and open item NFA-04 from the finite-overflow audit,
//! through a real helper. `make known-bugs` runs the ignored ones.
//!
//! The levels are near `f32::MAX` (3.4e38), where a finite sum or product
//! stops being an `f32`. The rate, block and planes bound these regressions only.

use super::*;

const RATE: u32 = 48_000;
const FRAMES: usize = 256;

fn block(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_until(left, right, Instant::now() + Duration::from_secs(5));
}

fn route(clap_id: &str, instrument: bool, fault: Option<&str>) -> PluginRoute {
    PluginRoute {
        clap_id: clap_id.into(),
        path: plugin_dylib(),
        instrument,
        fault: fault.map(str::to_owned),
        state: None,
    }
}

fn constant(track: Id, level: f32, gain: f32) -> LoadedClip {
    let samples: Arc<[f32]> = vec![level; RATE as usize].into();
    LoadedClip {
        track: track.raw(),
        start_sample: 0,
        samples: Arc::clone(&samples),
        right: Some(samples),
        window: None,
        gain,
        fade_in: 0,
        fade_out: 0,
    }
}

fn auto_mute(
    control: &echode_engine::PlaybackControl,
    track: Id,
) -> Option<echode_engine::AutoMute> {
    control
        .meters()
        .into_iter()
        .find(|meter| meter.track == track.raw())
        .unwrap()
        .auto_mute()
}

/// What the audit confirmed (ADR 0282): an instrument that is only loud, at
/// a finite 2.0e38, is not a faulty device. Its track is not auto-muted and
/// the export is not refused in its name.
#[test]
fn a_loud_finite_instrument_is_not_a_faulty_device() {
    let (project, track) = Project::new().add_track("loud instrument");
    let project = project
        .add_clap_device(track, Id::generate(), "dev.echode.test-instrument")
        .unwrap();
    let routes = [route("dev.echode.test-instrument", true, Some("huge"))];
    let (mut control, mut audio) =
        playback_with_plugins(&project, RATE, FRAMES, &host_bin(), &routes).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    for _ in 0..2 {
        block(&mut audio, &mut left, &mut right);
        control.service();
    }
    assert_eq!(left[FRAMES - 1], 2.0e38);
    assert!(auto_mute(&control, track).is_none());
    assert!(control.plugin_failure().is_none());
}

/// NFA-05: host overflow is attributed to the track's mix, not a healthy
/// instrument or downstream effect. Shapes and block lengths bound this test only.
#[test]
fn audit_nfa05_an_instrument_is_not_blamed_for_the_sum_with_its_tracks_clip() {
    for clap_id in ["dev.echode.test-instrument", "dev.echode.test-mono"] {
        for child_source in [false, true] {
            for frames in [1, 7, FRAMES] {
                let (project, track) = Project::new().add_track("instrument and clip");
                let (project, source) = if child_source {
                    project.add_child(track, "child source").unwrap()
                } else {
                    (project, track)
                };
                let project = project
                    .add_clap_device(track, Id::generate(), clap_id)
                    .unwrap()
                    .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
                    .unwrap();
                let routes = [
                    route(clap_id, true, Some("huge")),
                    route("dev.echode.test-gain", false, Some("delay:0")),
                ];
                let (mut control, mut audio) =
                    playback_with_plugins(&project, RATE, frames, &host_bin(), &routes).unwrap();
                // A finite instrument plus a finite clip overflows starting at
                // sample 11, including a mono instrument's stereo promotion.
                let mut clip = constant(source, 3.0e38, 1.0);
                clip.start_sample = 11;
                control.set_clip_loader(Arc::new(move |_, _| Ok(vec![clip.clone()])));
                control.replace(&project).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                for _ in 0..(512 / frames + 2) {
                    block(&mut audio, &mut left, &mut right);
                    control.service();
                    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
                }
                let mute = auto_mute(&control, track).expect("the host mix auto-mutes its track");
                assert_eq!(
                    mute.device, None,
                    "the track's auto-mute names the instrument"
                );
                assert_eq!(mute.at, 11, "the fault depends on the block length");
                assert_eq!(
                    control.render_failure(),
                    Some(echode_engine::RenderFailure::NonFiniteMix {
                        track: track.raw(),
                        at: 11
                    })
                );
                assert!(
                    control.plugin_failure().is_none(),
                    "a healthy device was blamed"
                );
                assert_eq!(control.status().plugin_missed_blocks, 0);
                assert!(!control.status().failed);
                assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));

                // Silence the source and start again: the same meter clears,
                // the finite instrument is not a fault, and export can resume.
                control.set_clip_loader(Arc::new(|_, _| Ok(Vec::new())));
                control.replace(&project).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                block(&mut audio, &mut left, &mut right);
                control.service();
                assert!(auto_mute(&control, track).is_none());
                assert!(control.render_failure().is_none());
            }
        }
    }
}

/// NFA-04: the engine accepts a clip whose gain overflows its samples
/// (`echode-audio-io` refuses it first). On a track whose first device is an
/// effect, the infinity is that effect's input, and the effect is then named
/// for the infinity it passes on.
#[test]
#[ignore = "NFA-04: an effect receives an infinity from a clip the engine accepted"]
fn audit_nfa04_an_effect_never_receives_an_infinity_from_placed_audio() {
    let (project, track) = Project::new().add_track("effect first");
    let effect = Id::generate();
    let project = project
        .add_clap_device(track, effect, "dev.echode.test-gain")
        .unwrap();
    let routes = [route("dev.echode.test-gain", false, None)];
    let (mut control, mut audio) =
        playback_with_plugins(&project, RATE, FRAMES, &host_bin(), &routes).unwrap();
    let clip = constant(track, 3.0e38, 2.0);
    control.set_clip_loader(Arc::new(move |_, _| Ok(vec![clip.clone()])));
    if control.replace(&project).is_err() {
        // Refusing the clip is a fix: nothing reaches the effect.
        return;
    }
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    for _ in 0..2 {
        block(&mut audio, &mut left, &mut right);
        control.service();
    }
    assert!(
        auto_mute(&control, track).is_none_or(|mute| mute.device != Some(effect.raw())),
        "the effect was given an infinity and named for returning it"
    );
}

/// NFA-07: a parent's instrument is delayed to meet its children (ADR 0339).
/// The host looked for a NaN after that line, so the line held the NaN the
/// instrument wrote, and it came out after the reset that releases the
/// auto-mute (ADR 0282): the track was muted again at once. The instrument's
/// own output is now checked before the line (ADR 0340), so the line never
/// holds one and the reset instrument's 0.5 is heard.
#[test]
fn audit_nfa07_a_delayed_instrument_sounds_again_once_its_auto_mute_is_released() {
    let (project, parent) = Project::new().add_track("parent instrument");
    let (project, child) = project.add_child(parent, "delayed child").unwrap();
    let project = project
        .add_clap_device(parent, Id::generate(), "dev.echode.test-instrument")
        .unwrap()
        .add_clap_device(child, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let routes = [
        route("dev.echode.test-instrument", true, Some("nan")),
        route("dev.echode.test-gain", false, Some("delay:64")),
    ];
    let (mut control, mut audio) =
        playback_with_plugins(&project, RATE, FRAMES, &host_bin(), &routes).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; FRAMES], vec![0.0; FRAMES]);
    // The second block carries the NaN; the rest hold the mute.
    for _ in 0..6 {
        block(&mut audio, &mut left, &mut right);
        control.service();
    }
    assert!(
        auto_mute(&control, parent).is_some(),
        "the fixture's NaN auto-mutes the parent"
    );
    assert_eq!(control.latency_samples(), 64, "the child delays the bus");

    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..6 {
        block(&mut audio, &mut left, &mut right);
        control.service();
    }
    assert!(
        auto_mute(&control, parent).is_none(),
        "the reset instrument is auto-muted again"
    );
    assert!(
        left.iter().all(|sample| (*sample - 0.5).abs() < 1.0e-6),
        "the instrument's 0.5 is not heard: {}",
        left[FRAMES - 1]
    );
}
