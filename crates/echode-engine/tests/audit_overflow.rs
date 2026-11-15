//! Items from docs/reviews/2026-11-13-finite-overflow-audit.md. The tests
//! hold what the audit confirmed already works and the items fixed since
//! (NFA-01 to NFA-03 in ADR 0359, NFA-04 in ADR 0369, NFA-06 in ADR 0344).
//!
//! Every clip here is finite. The levels are near `f32::MAX` (3.4e38) because
//! that is where a finite product or sum stops being an `f32`; they bound
//! these regressions only.

use std::sync::Arc;

use echode_engine::{
    AutoMute, CachedTrack, FinitePlanes, LoadedClip, MASTER_GUARD_DBFS, OfflineError, PlaneFault,
    PlaybackAudio, PlaybackCommand, PlaybackControl, PlaybackError, PlaybackOptions, RenderFailure,
    playback, render_offline_blocks_with_clips,
};
use echode_model::{DeviceKind, Id, ParamKey, Project};

const RATE: u32 = 48_000;
const BLOCK: usize = 64;
/// Four blocks. The fader's 5 ms ramp is settled from the first frame because
/// playback starts from a stopped transport.
const FRAMES: usize = 256;

fn stereo(track: Id, samples: Vec<f32>) -> LoadedClip {
    LoadedClip::new(
        track.raw(),
        0,
        FinitePlanes::new(samples.clone().into(), Some(samples.into()))
            .unwrap()
            .into(),
    )
}

fn constant(track: Id, level: f32) -> LoadedClip {
    stereo(track, vec![level; FRAMES])
}

fn set_gain(project: &Project, track: Id, gain: f32) -> Project {
    let parameter = project
        .tracks()
        .iter()
        .find(|candidate| candidate.id == track)
        .unwrap()
        .devices
        .iter()
        .find(|device| device.kind == DeviceKind::Mixer)
        .unwrap()
        .parameters
        .iter()
        .find(|parameter| parameter.key == ParamKey::Gain)
        .unwrap()
        .id;
    project.set_parameter(parameter, gain).unwrap()
}

/// The export path of a project without plugins: interleaved stereo.
fn exported(project: &Project, clips: &[LoadedClip]) -> Result<Vec<f32>, OfflineError<()>> {
    let mut output = Vec::new();
    render_offline_blocks_with_clips(project, RATE, FRAMES as u32, BLOCK as u32, clips, |block| {
        output.extend_from_slice(block);
        Ok::<_, ()>(())
    })?;
    Ok(output)
}

/// The typed refusal of an export, or a panic naming what it did instead.
fn refused(result: Result<Vec<f32>, OfflineError<()>>) -> RenderFailure {
    match result {
        Err(OfflineError::Render(failure)) => failure,
        Err(error) => panic!("refused for another reason: {error:?}"),
        Ok(output) => panic!("exported; the first sample is {}", output[0]),
    }
}

/// The device path: the master guard is on, as `echode-audio-io` sets it.
fn heard(project: &Project, clips: &[LoadedClip]) -> (Vec<f32>, PlaybackControl, PlaybackAudio) {
    let (mut control, mut audio) = playback(
        project,
        RATE,
        BLOCK,
        PlaybackOptions::default().guard_output(),
    )
    .unwrap();
    let loaded = clips.to_vec();
    control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let mut output = Vec::new();
    for _ in 0..FRAMES / BLOCK {
        audio.render(&mut left, &mut right);
        control.service();
        output.extend(left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]));
    }
    (output, control, audio)
}

fn silent(samples: &[f32]) -> bool {
    samples.iter().all(|sample| *sample == 0.0)
}

fn peak(control: &PlaybackControl, track: Id) -> f32 {
    control
        .meters()
        .into_iter()
        .find(|meter| meter.track == track.raw())
        .unwrap()
        .peak()
}

/// NFA-01: 3.0e38 through a fader at 2 is 6.0e38, which is not an `f32`.
/// It used to be written as 0, so the export succeeded with silence. The
/// product reaches the f64 master, and the export is refused where it does.
#[test]
fn audit_nfa01_an_export_does_not_write_silence_where_the_fader_overflows() {
    let (project, track) = Project::new().add_track("loud");
    let project = set_gain(&project, track, 2.0);
    let failure = refused(exported(&project, &[constant(track, 3.0e38)]));
    assert_eq!(failure, RenderFailure::NonFiniteMaster { at: 0 });
    assert!(failure.to_string().contains("master: audio sum produced"));
}

/// NFA-01: the same track on a device. Nothing is heard, and now the meter
/// shows the full range and the master auto-mute says why.
#[test]
fn audit_nfa01_a_device_reports_a_fader_overflow() {
    let (project, track) = Project::new().add_track("loud");
    let project = set_gain(&project, track, 2.0);
    let (output, control, _audio) = heard(&project, &[constant(track, 3.0e38)]);
    assert!(silent(&output), "nothing past the guard reaches the device");
    assert_eq!(peak(&control, track), f32::MAX);
    let mute = control.status().master_auto_mute.expect("auto-muted");
    assert_eq!((mute.peak, mute.at), (f32::INFINITY, 0));
    assert_eq!(
        control.render_failure(),
        Some(RenderFailure::NonFiniteMaster { at: 0 })
    );
}

/// NFA-02: two tracks at 2.0e38 sum to 4.0e38 in the f64 master, and the
/// narrowing to `f32` wrote 0. The export succeeded with silence; now it is
/// refused at the first sample that does not fit.
#[test]
fn audit_nfa02_an_export_does_not_write_silence_where_the_master_overflows() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let failure = refused(exported(&project, &clips));
    assert_eq!(failure, RenderFailure::NonFiniteMaster { at: 0 });
}

/// NFA-02: on a device both meters show 2.0e38 and the master is silent.
/// The auto-mute never fired, as the guard ran after the narrowing; it now
/// sees the infinity the narrowing makes.
#[test]
fn audit_nfa02_a_device_auto_mutes_a_master_that_overflows() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let (output, control, _audio) = heard(&project, &clips);
    assert!(silent(&output), "nothing past the guard reaches the device");
    assert_eq!(peak(&control, first), 2.0e38);
    let mute = control
        .status()
        .master_auto_mute
        .expect("a master 100 dB past the guard did not auto-mute");
    assert_eq!((mute.peak, mute.at), (f32::INFINITY, 0));
}

/// NFA-02: the export is refused at the sample where the sum stops fitting,
/// and the blocks before it are passed on unchanged: the value is a fault,
/// not a level, and the sum below `f32::MAX` is kept.
#[test]
fn an_export_is_refused_from_the_first_sample_whose_sum_does_not_fit() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    // Sums to 3.0e38 for 150 frames, then to 4.0e38.
    let rising: Vec<f32> = (0..FRAMES)
        .map(|frame| if frame < 150 { 1.5e38 } else { 2.0e38 })
        .collect();
    let clips = [stereo(first, rising.clone()), stereo(second, rising)];
    let mut passed = Vec::new();
    let result = render_offline_blocks_with_clips(
        &project,
        RATE,
        FRAMES as u32,
        BLOCK as u32,
        &clips,
        |block| {
            passed.extend_from_slice(block);
            Ok::<_, ()>(())
        },
    );
    assert!(matches!(
        result,
        Err(OfflineError::Render(RenderFailure::NonFiniteMaster {
            at: 150
        }))
    ));
    // The two whole blocks before the fault; the block holding it is not passed on.
    assert_eq!(passed.len(), 2 * BLOCK * 2);
    assert!(passed.iter().all(|sample| *sample == 3.0e38));
}

/// What the audit confirmed: when an overflow decays into the `f32` range,
/// its first representable sample is still past the guard, so no device
/// sample is ever past the guard, whether the master sum or one fader's
/// product was what overflowed. The guard used to see 0 for the overflow and
/// mute from that first representable sample; since NFA-02 it sees the
/// infinity and mutes from the first sample that did not fit.
#[test]
fn a_device_never_hears_an_overflow_decay_into_range() {
    // Falls from 3.0e38 to 0; doubled, it starts at 6.0e38.
    let ramp: Vec<f32> = (0..FRAMES)
        .map(|frame| 3.0e38 * (1.0 - frame as f32 / (FRAMES - 1) as f32))
        .collect();
    let first_in_range = ramp
        .iter()
        .position(|sample| ((f64::from(*sample) * 2.0) as f32).is_finite())
        .unwrap();
    assert!(first_in_range > 0, "the fixture starts past f32::MAX");
    let guard = 10f32.powf(MASTER_GUARD_DBFS / 20.0);

    let (summed, first) = Project::new().add_track("first");
    let (summed, second) = summed.add_track("second");
    let sum_clips = vec![stereo(first, ramp.clone()), stereo(second, ramp.clone())];
    let (faded, track) = Project::new().add_track("loud");
    let faded = set_gain(&faded, track, 2.0);
    let fader_clips = vec![stereo(track, ramp.clone())];

    for (name, project, clips) in [
        ("master sum", &summed, &sum_clips),
        ("fader product", &faded, &fader_clips),
    ] {
        let (output, control, _audio) = heard(project, clips);
        assert!(
            output.iter().all(|sample| sample.abs() <= guard),
            "{name}: a sample past the guard reached the device"
        );
        let mute = control.status().master_auto_mute.expect("auto-muted");
        assert_eq!((mute.peak, mute.at), (f32::INFINITY, 0), "{name}");
    }
}

/// NFA-03: two children at 2.0e38 enter a parent whose fader is 1/4. The
/// children's sum is narrowed into the parent's `f32` bus before its fader,
/// and became 0 there, so the export succeeded with silence. The audit's
/// reproduction expected the 1.0e38 the fader would bring back; the bus stays
/// `f32`, the form its plugins receive, so the sum is the parent's fault
/// instead, and the export is refused naming the parent (ADR 0359).
#[test]
fn audit_nfa03_a_folder_bus_past_the_f32_range_refuses_the_export_naming_its_parent() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, first) = project.add_child(folder, "first").unwrap();
    let (project, second) = project.add_child(folder, "second").unwrap();
    let project = set_gain(&project, folder, 0.25);
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let failure = refused(exported(&project, &clips));
    assert_eq!(
        failure,
        RenderFailure::NonFiniteMix {
            track: folder.raw(),
            at: 0
        }
    );
}

/// NFA-03 on a device: the parent auto-mutes without naming a device, the
/// children's meters still read, and a track outside the folder plays on.
#[test]
fn a_device_auto_mutes_a_folder_whose_bus_overflows_and_plays_the_rest() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, first) = project.add_child(folder, "first").unwrap();
    let (project, second) = project.add_child(folder, "second").unwrap();
    let (project, outside) = project.add_track("outside");
    let project = set_gain(&project, folder, 0.25);
    let clips = [
        constant(first, 2.0e38),
        constant(second, 2.0e38),
        constant(outside, 0.25),
    ];
    let (output, control, _audio) = heard(&project, &clips);
    assert!(
        output.iter().all(|sample| *sample == 0.25),
        "only the outside track is heard; the first sample is {}",
        output[0]
    );
    let mute = control
        .meters()
        .into_iter()
        .find(|meter| meter.track == folder.raw())
        .and_then(|meter| meter.auto_mute());
    assert_eq!(
        mute,
        Some(AutoMute {
            device: None,
            at: 0
        })
    );
    assert_eq!(peak(&control, first), 2.0e38);
    assert!(control.status().master_auto_mute.is_none());
    assert_eq!(
        control.render_failure(),
        Some(RenderFailure::NonFiniteMix {
            track: folder.raw(),
            at: 0
        })
    );
}

/// NFA-04: 3.0e38 at a clip gain of 2 is an infinity in the clip plane, and
/// the fade-in's first frame multiplies it by 0 into a NaN. `echode-audio-io`
/// refused this clip and the engine took it; since NFA-02 the master's NaN
/// refused the render instead. The engine's entrance now refuses the clip by
/// the peak of the frames it plays, naming the placement, before any block
/// is rendered, and the device path keeps the plan it had (ADR 0369).
#[test]
fn audit_nfa04_the_engine_refuses_a_clip_whose_gain_overflows_its_samples() {
    let (project, track) = Project::new().add_track("clip");
    let mut clip = constant(track, 3.0e38);
    clip.start_sample = 17;
    clip.gain = 2.0;
    clip.fade_in = 8;
    let overflowed = |result: Result<(), PlaybackError>| {
        assert!(
            matches!(
                result,
                Err(PlaybackError::ClipGainOverflow { track: refused, start_sample: 17 })
                    if refused == track.raw()
            ),
            "the engine accepted the clip: {result:?}"
        );
    };
    let mut blocks = 0;
    let result = render_offline_blocks_with_clips(
        &project,
        RATE,
        FRAMES as u32,
        BLOCK as u32,
        std::slice::from_ref(&clip),
        |_| {
            blocks += 1;
            Ok::<_, ()>(())
        },
    );
    overflowed(match result {
        Err(OfflineError::Playback(error)) => Err(error),
        other => panic!("refused for another reason: {other:?}"),
    });
    assert_eq!(blocks, 0, "a block was rendered before the refusal");

    let (mut control, _audio) = playback(
        &project,
        RATE,
        BLOCK,
        PlaybackOptions::default().guard_output(),
    )
    .unwrap();
    let loaded = clip.clone();
    control.set_clip_loader(Arc::new(move |_, _| Ok(vec![loaded.clone()])));
    overflowed(control.replace(&project));
    assert!(!control.status().failed);

    // At the largest gain its peak allows, the same clip plays.
    clip.gain = 1.0;
    let output = exported(&project, &[clip]).unwrap();
    // The fade-in reaches full level on its eighth frame, frame 24.
    assert_eq!(output[2 * (17 + 7)], 3.0e38);
}

/// NFA-04: a cache plane with a NaN was staged and played as silence. A
/// cached track now holds [`FinitePlanes`], which refuse the NaN when they
/// are made, so no cache the engine is given holds one (ADR 0369).
#[test]
fn audit_nfa04_a_cached_track_cannot_hold_a_nan() {
    let (project, track) = Project::new().add_track("cached");
    let mut samples = vec![0.25; FRAMES];
    samples[10] = f32::NAN;
    assert_eq!(
        FinitePlanes::new(samples.into(), None).unwrap_err(),
        PlaneFault::NonFinite { frame: 10 }
    );
    let (mut control, _audio) =
        playback(&project, RATE, BLOCK, PlaybackOptions::default()).unwrap();
    control
        .set_cached_tracks(&[CachedTrack {
            track: track.raw(),
            sample_rate: RATE,
            start_sample: 0,
            planes: FinitePlanes::new(vec![0.25; FRAMES].into(), None).unwrap(),
        }])
        .unwrap();
}

/// NFA-06: the sample audition is a second device output with no guard. A
/// finite file 40 dB past full scale reaches the device as it is, where the
/// master would have auto-muted (ADR 0282).
#[test]
fn audit_nfa06_an_audition_past_the_guard_does_not_reach_the_device() {
    use echode_engine::{AuditionSound, audition_pair};
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    control.play(AuditionSound::new(vec![100.0; FRAMES].into(), None).unwrap());
    let (mut left, mut right) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    audio.mix(&mut left, &mut right);
    let guard = 10f32.powf(MASTER_GUARD_DBFS / 20.0);
    assert!(
        left.iter()
            .chain(&right)
            .all(|sample| sample.abs() <= guard),
        "the device was given {}",
        left[0]
    );
}

/// NFA-06: replacing a sound fades the old one under the new one, and the
/// two are added in `f32`. Two finite sounds can leave an infinity in the
/// device buffer.
#[test]
fn audit_nfa06_an_audition_never_writes_an_infinity() {
    use echode_engine::{AuditionSound, audition_pair};
    let (mut control, mut audio) = audition_pair(RATE).unwrap();
    let sound = || AuditionSound::new(vec![3.0e38; RATE as usize].into(), None).unwrap();
    control.play(sound());
    let (mut left, mut right) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    audio.mix(&mut left, &mut right);
    control.play(sound());
    left.fill(0.0);
    right.fill(0.0);
    audio.mix(&mut left, &mut right);
    assert!(
        left.iter().chain(&right).all(|sample| sample.is_finite()),
        "the device buffer holds an infinity"
    );
}
