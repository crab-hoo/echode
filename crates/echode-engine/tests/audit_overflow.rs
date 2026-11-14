//! Open items from docs/reviews/2026-11-13-finite-overflow-audit.md. Each
//! ignored test fails until its item is fixed; `make known-bugs` runs them.
//! Remove the `#[ignore]` with the fix. The tests that are not ignored hold
//! what the audit confirmed already works.
//!
//! Every clip here is finite. The levels are near `f32::MAX` (3.4e38) because
//! that is where a finite product or sum stops being an `f32`; they bound
//! these regressions only.

use std::sync::Arc;

use echode_engine::{
    CachedTrack, LoadedClip, MASTER_GUARD_DBFS, PlaybackAudio, PlaybackCommand, PlaybackControl,
    PlaybackOptions, playback, render_offline_blocks_with_clips,
};
use echode_model::{DeviceKind, Id, ParamKey, Project};

const RATE: u32 = 48_000;
const BLOCK: usize = 64;
/// Four blocks. The fader's 5 ms ramp is settled from the first frame because
/// playback starts from a stopped transport.
const FRAMES: usize = 256;

fn stereo(track: Id, samples: Vec<f32>) -> LoadedClip {
    LoadedClip {
        track: track.raw(),
        start_sample: 0,
        samples: samples.clone().into(),
        right: Some(samples.into()),
        window: None,
        gain: 1.0,
        fade_in: 0,
        fade_out: 0,
    }
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
fn exported(project: &Project, clips: &[LoadedClip]) -> Result<Vec<f32>, String> {
    let mut output = Vec::new();
    render_offline_blocks_with_clips(project, RATE, FRAMES as u32, BLOCK as u32, clips, |block| {
        output.extend_from_slice(block);
        Ok::<_, String>(())
    })
    .map_err(|error| error.to_string())?;
    Ok(output)
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
/// `scaled` writes 0 for it, so the export succeeds with silence.
#[test]
#[ignore = "NFA-01: a fader product past f32::MAX is exported as silence"]
fn audit_nfa01_an_export_does_not_write_silence_where_the_fader_overflows() {
    let (project, track) = Project::new().add_track("loud");
    let project = set_gain(&project, track, 2.0);
    let result = exported(&project, &[constant(track, 3.0e38)]);
    assert!(
        !result.as_deref().is_ok_and(silent),
        "the export succeeded and wrote 0 for every sample"
    );
}

/// NFA-01: the same track on a device. Nothing is heard, which is safe, but
/// neither the meter nor the master auto-mute says why.
#[test]
#[ignore = "NFA-01: a fader product past f32::MAX is silenced without a record"]
fn audit_nfa01_a_device_reports_a_fader_overflow() {
    let (project, track) = Project::new().add_track("loud");
    let project = set_gain(&project, track, 2.0);
    let (output, control, _audio) = heard(&project, &[constant(track, 3.0e38)]);
    assert!(silent(&output), "nothing past the guard reaches the device");
    assert!(
        control.status().master_auto_mute.is_some() || peak(&control, track) > 0.0,
        "silence with an empty meter and no auto-mute"
    );
}

/// NFA-02: two tracks at 2.0e38 sum to 4.0e38 in the f64 master, and the
/// narrowing to `f32` writes 0. The export succeeds with silence.
#[test]
#[ignore = "NFA-02: a master sum past f32::MAX is exported as silence"]
fn audit_nfa02_an_export_does_not_write_silence_where_the_master_overflows() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let result = exported(&project, &clips);
    assert!(
        !result.as_deref().is_ok_and(silent),
        "the export succeeded and wrote 0 for every sample"
    );
}

/// NFA-02: on a device both meters show 2.0e38 and the master is silent,
/// but the master auto-mute never fires: the guard runs after the narrowing.
#[test]
#[ignore = "NFA-02: a master sum past f32::MAX is silenced without an auto-mute"]
fn audit_nfa02_a_device_auto_mutes_a_master_that_overflows() {
    let (project, first) = Project::new().add_track("first");
    let (project, second) = project.add_track("second");
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let (output, control, _audio) = heard(&project, &clips);
    assert!(silent(&output), "nothing past the guard reaches the device");
    assert_eq!(peak(&control, first), 2.0e38);
    assert!(
        control.status().master_auto_mute.is_some(),
        "a master 100 dB past the guard did not auto-mute"
    );
}

/// What the audit confirmed: when an overflow decays into the `f32` range,
/// its first representable sample is still past the guard, so the guard mutes
/// from that sample. No device sample is ever past the guard, whether the
/// master sum or one fader's product was what overflowed.
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
        assert_eq!(mute.at, first_in_range as i64, "{name}");
    }
}

/// NFA-03: two children at 2.0e38 enter a parent whose fader is 1/4. The
/// master is 1.0e38, an ordinary `f32`, but the children's sum is narrowed to
/// `f32` before the parent's fader and becomes 0 there.
#[test]
#[ignore = "NFA-03: a folder bus past f32::MAX is exported as silence"]
fn audit_nfa03_a_folder_bus_keeps_a_sum_its_fader_brings_back_into_range() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, first) = project.add_child(folder, "first").unwrap();
    let (project, second) = project.add_child(folder, "second").unwrap();
    let project = set_gain(&project, folder, 0.25);
    let clips = [constant(first, 2.0e38), constant(second, 2.0e38)];
    let output = exported(&project, &clips).unwrap();
    assert!(
        output.iter().all(|sample| *sample == 1.0e38),
        "expected 1.0e38 on every sample, the first is {}",
        output[0]
    );
}

/// NFA-04: 3.0e38 at a clip gain of 2 is an infinity in the clip plane, and
/// the fade-in's first frame multiplies it by 0 into a NaN. `echode-audio-io`
/// refuses this clip before it gets here; the engine takes it and plays
/// silence.
#[test]
#[ignore = "NFA-04: the engine accepts a clip whose gain overflows its samples"]
fn audit_nfa04_the_engine_refuses_a_clip_whose_gain_overflows_its_samples() {
    let (project, track) = Project::new().add_track("clip");
    let mut clip = constant(track, 3.0e38);
    clip.gain = 2.0;
    clip.fade_in = 8;
    let result = exported(&project, &[clip]);
    assert!(
        !result.as_deref().is_ok_and(silent),
        "the engine accepted the clip and exported silence"
    );
}

/// NFA-04: a cache plane with a NaN is staged and plays as silence.
#[test]
#[ignore = "NFA-04: the engine accepts a cached track that holds a NaN"]
fn audit_nfa04_the_engine_refuses_a_cached_track_with_a_nan() {
    let (project, track) = Project::new().add_track("cached");
    let (mut control, _audio) =
        playback(&project, RATE, BLOCK, PlaybackOptions::default()).unwrap();
    let mut samples = vec![0.25; FRAMES];
    samples[10] = f32::NAN;
    let staged = control.set_cached_tracks(&[CachedTrack {
        track: track.raw(),
        sample_rate: RATE,
        start_sample: 0,
        samples: samples.into(),
        right: None,
    }]);
    assert!(staged.is_err(), "a cache with a NaN was staged");
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
