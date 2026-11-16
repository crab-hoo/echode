//! Product buffers for pre-fader sends, including a return that precedes its source.
use std::sync::Arc;

use echode_engine::{FinitePlanes, LoadedClip, PlaybackCommand, PlaybackOptions, playback};
use echode_model::{Id, ParamKey, Project};

use super::BLOCKS;

#[test]
fn sends_sum_before_return_faders_independently_of_blocks_under_rtsan() {
    let (project, parent) = Project::new().add_track("Folder");
    let (project, target) = project.add_track("Return");
    let (project, source) = project.add_track("Source");
    let project = project
        .set_parent(target, Some(parent))
        .unwrap()
        .add_send_with_id(Id::generate(), source, target, 0.5)
        .unwrap();
    let project = super::folder_mix::set_value(&project, source, ParamKey::Gain, 0.0);
    let project = super::folder_mix::set_value(&project, target, ParamKey::Gain, 0.5);
    // This length only bounds the fixture, not the product's buffers.
    let frames = 1031;
    let loaded = vec![LoadedClip::new(
        source.raw(),
        0,
        FinitePlanes::new(
            Arc::from(vec![0.25; frames]),
            Some(Arc::from(vec![-0.125; frames])),
        )
        .unwrap()
        .into(),
    )];
    for block in BLOCKS {
        let (mut control, mut audio) =
            playback(&project, 48_000, block as usize, PlaybackOptions::default()).unwrap();
        let clips = loaded.clone();
        control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
        control.replace(&project).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        for offset in (0..frames).step_by(block as usize) {
            let count = (frames - offset).min(block as usize);
            audio.render(&mut left[..count], &mut right[..count]);
            assert!(!control.status().failed);
            assert!(
                left[..count].iter().all(|&x| x == 0.0625),
                "send must reach the return before its fader: block {block}, {left:?}"
            );
            assert!(right[..count].iter().all(|&x| x == -0.03125));
        }
        echode_engine::render_offline_blocks_with_clips(
            &project,
            48_000,
            frames as u32,
            block,
            &loaded,
            |samples| {
                for pair in samples.chunks_exact(2) {
                    assert_eq!(pair, &[0.0625, -0.03125]);
                }
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    }
}

#[test]
fn sends_accumulate_in_double_precision_and_a_mute_closes_the_tap_under_rtsan() {
    let (project, target) = Project::new().add_track("Return");
    let (project, a) = project.add_track("A");
    let (project, b) = project.add_track("B");
    let (project, c) = project.add_track("C");
    let mut project = project;
    for source in [a, b, c] {
        project = project
            .add_send_with_id(Id::generate(), source, target, 1.0)
            .unwrap();
        project = super::folder_mix::set_value(&project, source, ParamKey::Gain, 0.0);
    }
    // Fixture-only finite cancellation: f32 addition would lose the middle 1.
    let frames = 1031;
    let loaded: Vec<_> = [(a, 1.0e10), (b, 1.0), (c, -1.0e10)]
        .into_iter()
        .map(|(track, value)| {
            LoadedClip::new(
                track.raw(),
                0,
                FinitePlanes::new(
                    Arc::from(vec![value; frames]),
                    Some(Arc::from(vec![value; frames])),
                )
                .unwrap()
                .into(),
            )
        })
        .collect();
    for muted in [false, true] {
        let case = super::folder_mix::set_value(&project, b, ParamKey::Mute, f32::from(muted));
        for block in BLOCKS {
            let (mut control, mut audio) =
                playback(&case, 48_000, block as usize, PlaybackOptions::default()).unwrap();
            let clips = loaded.clone();
            control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
            control.replace(&case).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            for offset in (0..frames).step_by(block as usize) {
                let count = (frames - offset).min(block as usize);
                audio.render(&mut left[..count], &mut right[..count]);
                assert!(
                    left[..count]
                        .iter()
                        .chain(&right[..count])
                        .all(|&sample| sample == if muted { 0.0 } else { 1.0 })
                );
                assert!(!control.status().failed);
            }
        }
    }
}
