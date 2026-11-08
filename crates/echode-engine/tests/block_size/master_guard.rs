//! A device output mutes a runaway master from its first sample past the
//! guard, whatever the block length, and keeps every value offline (ADR 0282).

use super::*;
use echode_engine::{MASTER_GUARD_DBFS, PlaybackCommand, PlaybackOptions, playback};

/// A stereo ramp from silence to +20 dBFS, opposite in phase. The length
/// bounds this regression only.
fn ramp(track: Id) -> (Vec<f32>, echode_engine::LoadedClip) {
    let left: Vec<f32> = (0..1003).map(|index| index as f32 / 100.0).collect();
    let right = left.iter().map(|sample| -sample).collect();
    (left.clone(), stereo_clip(track, left, right))
}

fn heard(
    project: &Project,
    block: usize,
    clip: &echode_engine::LoadedClip,
    guard: bool,
) -> (
    Vec<(f32, f32)>,
    echode_engine::PlaybackControl,
    echode_engine::PlaybackAudio,
) {
    let options = if guard {
        PlaybackOptions::default().guard_output()
    } else {
        PlaybackOptions::default()
    };
    let (mut control, mut audio) = playback(project, RATE, block, options).unwrap();
    let clips = vec![clip.clone()];
    control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = Vec::new();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    while output.len() < 1003 {
        let count = block.min(1003 - output.len());
        audio.render(&mut left[..count], &mut right[..count]);
        output.extend(
            left[..count]
                .iter()
                .copied()
                .zip(right[..count].iter().copied()),
        );
        control.service();
    }
    (output, control, audio)
}

#[test]
fn a_device_master_mutes_from_its_first_runaway_sample_for_every_block_size() {
    let (project, track) = Project::new().add_track("runaway");
    let (source, clip) = ramp(track);
    let guard = 10f32.powf(MASTER_GUARD_DBFS / 20.0);
    let first = source.iter().position(|sample| *sample > guard).unwrap();
    for block in BLOCKS {
        let (output, control, _) = heard(&project, block as usize, &clip, true);
        for (index, (left, right)) in output.iter().enumerate() {
            let expected = if index < first { source[index] } else { 0.0 };
            assert_eq!(
                (*left, *right),
                (expected, -expected),
                "block {block}, frame {index}"
            );
        }
        let mute = control
            .status()
            .master_auto_mute
            .expect("master auto-muted");
        assert_eq!(mute.at, first as i64, "block {block}");
        assert_eq!(mute.peak, source[first], "block {block}");
    }
}

#[test]
fn starting_playback_unmutes_the_master_and_offline_keeps_every_value() {
    let (project, track) = Project::new().add_track("runaway");
    let (source, clip) = ramp(track);
    let (offline, control, _) = heard(&project, 64, &clip, false);
    assert!(control.status().master_auto_mute.is_none());
    let expected: Vec<_> = source.iter().map(|sample| (*sample, -*sample)).collect();
    assert_eq!(offline, expected, "an export must not be muted or limited");

    let (_, mut control, mut audio) = heard(&project, 64, &clip, true);
    assert!(control.status().master_auto_mute.is_some());
    // Muted while stopped and while playing on, until playback starts again.
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    control.command(PlaybackCommand::Locate(0)).unwrap();
    audio.render(&mut left, &mut right);
    control.service();
    assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    assert!(control.status().master_auto_mute.is_some());
    control.command(PlaybackCommand::Play).unwrap();
    audio.render(&mut left, &mut right);
    control.service();
    assert!(control.status().master_auto_mute.is_none());
    assert_eq!(left, source[64..128], "playback is heard again at once");
}
