//! A sum that does not fit its `f32` plane is a fault from the same sample
//! whatever the block length, on a device and in an export (NFA-01 to NFA-03).
//!
//! Every value is finite. The levels near `f32::MAX` (3.4e38) are where a
//! product or a sum stops being an `f32`; they and the lengths bound these
//! regressions only.

use super::folder_mix::set_value;
use super::*;
use echode_engine::{
    AutoMute, LoadedClip, OfflineError, PlaybackCommand, PlaybackControl, PlaybackOptions,
    RenderFailure, playback, render_offline_blocks_with_clips,
};
use echode_model::ParamKey;

/// The first sample that does not fit. Not a multiple of any block length.
const FAULT: usize = 1001;
/// The overflow ends here; an auto-mute holds past it.
const CALM: usize = 2000;
/// Playback starts again here: a multiple of every block length.
const REPLAY: usize = 3584;
const FRAMES: usize = REPLAY + 600;
/// The fade of a fault and of its release: 5 ms (ADR 0245).
const FADE: usize = RATE as usize / 200;

/// `level` everywhere but `[FAULT, CALM)`, which holds `loud`.
fn bursting(track: Id, level: f32, loud: f32) -> LoadedClip {
    let samples: Vec<f32> = (0..FRAMES)
        .map(|frame| {
            if (FAULT..CALM).contains(&frame) {
                loud
            } else {
                level
            }
        })
        .collect();
    stereo_clip(track, samples.clone(), samples)
}

fn constant(track: Id, level: f32) -> LoadedClip {
    stereo_clip(track, vec![level; FRAMES], vec![level; FRAMES])
}

/// The device path with its guard, starting playback again at `REPLAY`.
fn heard(project: &Project, clips: &[LoadedClip], block: usize) -> (Vec<f32>, PlaybackControl) {
    let options = PlaybackOptions::default().guard_output();
    let (mut control, mut audio) = playback(project, RATE, block, options).unwrap();
    let loaded = clips.to_vec();
    control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
    control.replace(project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    let mut output = Vec::new();
    while output.len() < FRAMES {
        if output.len() == REPLAY {
            control.command(PlaybackCommand::Play).unwrap();
        }
        let count = block.min(FRAMES - output.len());
        audio.render(&mut left[..count], &mut right[..count]);
        control.service();
        for (left, right) in left[..count].iter().zip(&right[..count]) {
            assert_eq!(left, right, "the fixture is the same on both sides");
            output.push(*left);
        }
        if output.len() == REPLAY {
            assert!(
                control.render_failure().is_some(),
                "the fault holds until playback starts again"
            );
        }
    }
    (output, control)
}

/// The export path: what it passed on before it refused, and why.
fn exported(project: &Project, clips: &[LoadedClip], block: u32) -> (Vec<f32>, RenderFailure) {
    let mut passed = Vec::new();
    let result =
        render_offline_blocks_with_clips(project, RATE, FRAMES as u32, block, clips, |samples| {
            passed.extend_from_slice(samples);
            Ok::<_, ()>(())
        });
    let Err(OfflineError::Render(failure)) = result else {
        panic!("block {block}: the export was not refused for its sum: {result:?}");
    };
    (passed, failure)
}

/// NFA-01: a fader at 2 doubles 3.0e38 past the f32 range. The master mutes
/// from that sample and the export is refused there, never written as 0.
#[test]
fn a_fader_overflow_mutes_the_master_and_refuses_the_export_at_the_same_sample() {
    let (project, track) = Project::new().add_track("loud");
    let project = set_value(&project, track, ParamKey::Gain, 2.0);
    let clips = [bursting(track, 0.25, 3.0e38)];
    let mut reference = None;
    for block in BLOCKS {
        let (output, control) = heard(&project, &clips, block as usize);
        // Silent from the fault, and still at the replay: only a new Play
        // releases a master (ADR 0282), and the clip is quiet again by then.
        let expected = |frame: usize| {
            if !(FAULT..REPLAY).contains(&frame) {
                0.5
            } else {
                0.0
            }
        };
        for (frame, sample) in output.iter().enumerate() {
            assert_eq!(*sample, expected(frame), "block {block}, frame {frame}");
        }
        assert!(control.status().master_auto_mute.is_none(), "released");
        assert_eq!(reference.get_or_insert_with(|| output.clone()), &output);

        let (passed, failure) = exported(&project, &clips, block);
        assert_eq!(
            failure,
            RenderFailure::NonFiniteMaster { at: FAULT as i64 },
            "block {block}"
        );
        // Whole blocks before the one holding the fault, each sample intact.
        let kept = FAULT / block as usize * block as usize;
        assert_eq!(passed.len(), kept * 2, "block {block}");
        assert!(passed.iter().all(|sample| *sample == 0.5), "block {block}");
    }
}

/// NFA-03: two children at 2.0e38 sum past the folder's f32 bus. The folder
/// fades out from that sample like a faulted hosted track, stays muted after
/// the burst, and crosses back in when playback starts again. A track outside
/// the folder plays on throughout, and the export is refused naming the folder.
#[test]
fn a_folder_bus_overflow_mutes_its_parent_from_the_same_sample_and_returns_on_play() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, first) = project.add_child(folder, "first").unwrap();
    let (project, second) = project.add_child(folder, "second").unwrap();
    let (project, outside) = project.add_track("outside");
    let project = set_value(&project, folder, ParamKey::Gain, 0.25);
    let clips = [
        bursting(first, 0.25, 2.0e38),
        bursting(second, 0.25, 2.0e38),
        constant(outside, 0.125),
    ];
    // The folder alone is (0.25 + 0.25) / 4; with the outside track, 0.25.
    let (folder_level, outside_level) = (0.125f32, 0.125f32);
    let mut reference = None;
    for block in BLOCKS {
        let (output, control) = heard(&project, &clips, block as usize);
        for (frame, sample) in output.iter().enumerate() {
            let folder_part = sample - outside_level;
            match frame {
                _ if frame < FAULT => assert_eq!(*sample, folder_level + outside_level),
                // The fade starts from the last heard value and reaches 0.
                _ if frame == FAULT => assert_eq!(folder_part, folder_level),
                _ if (FAULT..FAULT + FADE).contains(&frame) => {
                    assert!((0.0..folder_level).contains(&folder_part), "frame {frame}")
                }
                _ if frame < REPLAY => assert_eq!(*sample, outside_level, "frame {frame}"),
                // Crossing back in over the same 5 ms, then heard as before.
                _ if frame == REPLAY => assert_eq!(*sample, outside_level),
                _ if frame < REPLAY + FADE => {
                    assert!((0.0..=folder_level).contains(&folder_part), "frame {frame}")
                }
                _ => assert_eq!(
                    *sample,
                    folder_level + outside_level,
                    "block {block}, frame {frame}"
                ),
            }
        }
        assert!(control.status().master_auto_mute.is_none());
        assert_eq!(reference.get_or_insert_with(|| output.clone()), &output);

        let (passed, failure) = exported(&project, &clips, block);
        assert_eq!(
            failure,
            RenderFailure::NonFiniteMix {
                track: folder.raw(),
                at: FAULT as i64
            },
            "block {block}"
        );
        let kept = FAULT / block as usize * block as usize;
        assert_eq!(passed.len(), kept * 2, "block {block}");
        assert!(
            passed
                .iter()
                .all(|sample| *sample == folder_level + outside_level),
            "block {block}"
        );
    }
}

/// The record of a folder's fault names no device and keeps the first fault.
#[test]
fn a_folder_bus_fault_is_recorded_once_without_a_device() {
    let (project, folder) = Project::new().add_track("folder");
    let (project, child) = project.add_child(folder, "child").unwrap();
    let (project, other) = project.add_child(folder, "other").unwrap();
    let clips = [bursting(child, 0.25, 2.0e38), bursting(other, 0.25, 2.0e38)];
    let options = PlaybackOptions::default().guard_output();
    let (mut control, mut audio) = playback(&project, RATE, 64, options).unwrap();
    let loaded = clips.to_vec();
    control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    for _ in 0..CALM / 64 + 1 {
        audio.render(&mut left, &mut right);
        control.service();
    }
    let meter = control
        .meters()
        .into_iter()
        .find(|meter| meter.track == folder.raw())
        .unwrap();
    assert_eq!(
        meter.auto_mute(),
        Some(AutoMute {
            device: None,
            at: FAULT as i64
        })
    );
    // The children's meters read the full range, not an empty bar.
    let child_peak = control
        .meters()
        .into_iter()
        .find(|meter| meter.track == child.raw())
        .unwrap()
        .peak();
    assert!(child_peak >= 2.0e38);
}
