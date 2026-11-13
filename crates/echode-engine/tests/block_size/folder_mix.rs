//! Folder gates are prepared off the callback, then use ordinary track ramps.
//!
//! A child reaches master through every ancestor's fader (ADR 0338), so a
//! child's sample is its level times the gains along its path. Each fader
//! that changes ramps linearly over 5 ms; nested changes multiply.
use std::sync::Arc;

use echode_engine::{
    LoadedClip, PlaybackCommand, PlaybackOptions, playback, render_offline_blocks_with_clips,
};
use echode_model::{DeviceKind, Id, ParamKey, Project};

use super::BLOCKS;

// Five tracks, these signal levels and this duration bound only this regression.
pub(super) const LEVELS: [f32; 5] = [1.0 / 64.0, 2.0 / 64.0, 4.0 / 64.0, 8.0 / 64.0, 16.0 / 64.0];
pub(super) const SECTION: usize = 1027;
/// The fixture's parents: root, inner (root), leaf (inner), peer (root), outside.
pub(super) const TREE: [Option<usize>; 5] = [None, Some(0), Some(1), Some(0), None];

pub(super) fn fixture() -> (Project, [Id; 5]) {
    let (project, root) = Project::new().add_track("folder");
    let (project, inner) = project.add_child(root, "nested folder").unwrap();
    let (project, leaf) = project.add_child(inner, "nested source").unwrap();
    let (project, peer) = project.add_child(root, "sibling source").unwrap();
    let (project, outside) = project.add_track("outside source");
    (project, [root, inner, leaf, peer, outside])
}

fn set(project: &Project, track: Id, key: ParamKey, value: bool) -> Project {
    set_value(project, track, key, f32::from(value))
}

pub(super) fn set_value(project: &Project, track: Id, key: ParamKey, value: f32) -> Project {
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
        .find(|parameter| parameter.key == key)
        .unwrap()
        .id;
    project.set_parameter(parameter, value).unwrap()
}

pub(super) fn clips(ids: [Id; 5], frames: usize) -> Vec<LoadedClip> {
    ids.into_iter()
        .zip(LEVELS)
        .map(|(track, level)| LoadedClip {
            track: track.raw(),
            start_sample: 0,
            samples: vec![level; frames].into(),
            right: Some(vec![-level / 2.0; frames].into()),
            window: None,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
        })
        .collect()
}

fn sum(audible: [bool; 5]) -> f32 {
    LEVELS
        .into_iter()
        .zip(audible)
        .filter_map(|(level, audible)| audible.then_some(level))
        .sum()
}

/// What a track's meter shows at rest: every heard level on its bus. A track
/// whose fader is closed has nothing heard below it.
fn bus_sum(audible: [bool; 5], track: usize) -> f32 {
    (0..5)
        .filter(|&source| {
            let mut at = Some(source);
            while let Some(current) = at {
                if current == track {
                    return audible[source];
                }
                at = TREE[current];
            }
            false
        })
        .map(|source| LEVELS[source])
        .sum()
}

/// One fixture state: each track's parent, fader gain, whether its fader is
/// open, and whether its own audio is heard.
#[derive(Clone, Copy)]
pub(super) struct State {
    pub(super) parents: [Option<usize>; 5],
    pub(super) gains: [f32; 5],
    pub(super) open: [bool; 5],
    pub(super) own: [bool; 5],
}

/// The left sample `fraction` of the way through the 5 ms ramps from
/// `before` to `after`, built from each track's level and the gates on its
/// path, independently of the engine's bus code.
pub(super) fn ramped(before: State, after: State, fraction: f64) -> f64 {
    let lerp = |from: bool, to: bool| {
        let (from, to) = (f64::from(u8::from(from)), f64::from(u8::from(to)));
        from + (to - from) * fraction
    };
    (0..5)
        .map(|source| {
            let mut sample =
                f64::from(LEVELS[source]) * lerp(before.own[source], after.own[source]);
            let mut at = Some(source);
            while let Some(track) = at {
                sample *=
                    f64::from(after.gains[track]) * lerp(before.open[track], after.open[track]);
                at = after.parents[track];
            }
            sample
        })
        .sum()
}

#[test]
fn nested_folder_gates_match_offline_audio_and_post_fader_meters_under_rtsan() {
    let (project, ids @ [root, inner, leaf, _, outside]) = fixture();
    let root_solo = set(&project, root, ParamKey::Solo, true);
    let inner_solo = set(&project, inner, ParamKey::Solo, true);
    let leaf_solo = set(&project, leaf, ParamKey::Solo, true);
    let cases = [
        (project.clone(), [true; 5]),
        (
            set(&project, root, ParamKey::Mute, true),
            [false, false, false, false, true],
        ),
        (root_solo.clone(), [true, true, true, true, false]),
        (inner_solo.clone(), [false, true, true, false, false]),
        (leaf_solo.clone(), [false, false, true, false, false]),
        (
            set(&inner_solo, outside, ParamKey::Solo, true),
            [false, true, true, false, true],
        ),
        (
            set(&root_solo, inner, ParamKey::Mute, true),
            [true, false, false, true, false],
        ),
        (set(&inner_solo, root, ParamKey::Mute, true), [false; 5]),
        (
            set(&root_solo, leaf, ParamKey::Mute, true),
            [true, true, false, true, false],
        ),
        (set(&leaf_solo, leaf, ParamKey::Mute, true), [false; 5]),
    ];
    let loaded = clips(ids, SECTION);
    for (case, audible) in cases {
        for rate in [48_000, 96_000] {
            for block in BLOCKS {
                let (mut control, mut audio) =
                    playback(&case, rate, block as usize, PlaybackOptions::default()).unwrap();
                let loaded = loaded.clone();
                let offline_clips = loaded.clone();
                control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
                control.replace(&case).unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let mut left = vec![0.0; block as usize];
                let mut right = left.clone();
                let mut heard = Vec::new();
                for offset in (0..SECTION).step_by(block as usize) {
                    let count = (block as usize).min(SECTION - offset);
                    audio.render(&mut left[..count], &mut right[..count]);
                    for (&left, &right) in left.iter().zip(&right).take(count) {
                        assert_eq!(
                            (left, right),
                            (sum(audible), -sum(audible) / 2.0),
                            "folder audible set {audible:?}, rate {rate}, block {block}"
                        );
                        heard.extend_from_slice(&[left, right]);
                    }
                }
                for (index, id) in ids.into_iter().enumerate() {
                    let meter = control
                        .meters()
                        .into_iter()
                        .find(|meter| meter.track == id.raw())
                        .unwrap();
                    assert_eq!(meter.peak(), bus_sum(audible, index), "track {index}");
                }
                let mut offline = Vec::new();
                render_offline_blocks_with_clips(
                    &case,
                    rate,
                    SECTION as u32,
                    block,
                    &offline_clips,
                    |samples| {
                        offline.extend_from_slice(samples);
                        Ok::<_, ()>(())
                    },
                )
                .unwrap();
                assert_eq!(heard, offline);
                assert!(!control.status().failed);
            }
        }
    }
}

#[test]
fn live_folder_edits_and_reparenting_ramp_independently_of_block_size_under_rtsan() {
    let (project, ids @ [root, inner, leaf, _, _]) = fixture();
    let solo = set(&project, root, ParamKey::Solo, true);
    let nested_mute = set(&solo, inner, ParamKey::Mute, true);
    let detached = solo.set_parent(leaf, None).unwrap();
    let restored = detached.set_parent(leaf, Some(inner)).unwrap();
    let state = |parents, open| State {
        parents,
        gains: [1.0; 5],
        open,
        own: [true; 5],
    };
    // A fader closes when its own path is silent, so a muted folder closes its
    // children too, and their audio fades through both ramps at once.
    let cases = [
        (project.clone(), state(TREE, [true; 5])),
        (
            set(&project, root, ParamKey::Mute, true),
            state(TREE, [false, false, false, false, true]),
        ),
        (solo, state(TREE, [true, true, true, true, false])),
        (nested_mute, state(TREE, [true, false, false, true, false])),
        // The detached leaf is a top-level track outside the solo. Its fader
        // was already closed under the muted folder, so it never plays alone.
        (
            detached,
            state(
                [None, Some(0), None, Some(0), None],
                [true, true, false, true, false],
            ),
        ),
        (restored, state(TREE, [true, true, true, true, false])),
        (project, state(TREE, [true; 5])),
    ];
    let loaded = clips(ids, SECTION * cases.len());
    for rate in [48_000, 96_000] {
        let mut reference = None;
        for block in BLOCKS {
            let (mut control, mut audio) = playback(
                &cases[0].0,
                rate,
                block as usize,
                PlaybackOptions::default(),
            )
            .unwrap();
            let loaded = loaded.clone();
            control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
            control.replace(&cases[0].0).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut left = vec![0.0; block as usize];
            let mut right = left.clone();
            let mut heard = Vec::new();
            let mut previous = cases[0].1;
            for (index, (case, target)) in cases.iter().enumerate() {
                if index != 0 {
                    control.replace(case).unwrap();
                }
                for offset in (0..SECTION).step_by(block as usize) {
                    let count = (block as usize).min(SECTION - offset);
                    audio.render(&mut left[..count], &mut right[..count]);
                    control.service();
                    for (frame, (&left, &right)) in left.iter().zip(&right).take(count).enumerate()
                    {
                        let ramp = rate as usize * 5 / 1000;
                        let fraction = ((offset + frame + 1) as f64 / ramp as f64).min(1.0);
                        let expected = ramped(previous, *target, fraction);
                        assert!(
                            (f64::from(left) - expected).abs() < 1.0e-6,
                            "section {index}, frame {}, block {block}: {left} != {expected}",
                            offset + frame
                        );
                        assert_eq!(right, -left / 2.0);
                        heard.push((left, right));
                    }
                }
                previous = *target;
            }
            assert!(!control.status().failed);
            if let Some(reference) = &reference {
                assert_eq!(&heard, reference, "rate {rate}, block {block}");
            } else {
                reference = Some(heard);
            }
        }
    }
}

#[test]
fn cached_audio_and_gain_previews_keep_ancestor_gates_under_rtsan() {
    use echode_engine::CachedTrack;

    let (project, [root, _, leaf, _, _]) = fixture();
    let muted = set(&project, root, ParamKey::Mute, true);
    let soloed = set(&project, root, ParamKey::Solo, true);
    for (case, expected) in [(muted, 0.0), (soloed, 0.5)] {
        let (mut control, mut audio) =
            playback(&case, 48_000, 64, PlaybackOptions::default()).unwrap();
        control
            .set_cached_tracks(&[CachedTrack {
                track: leaf.raw(),
                sample_rate: 48_000,
                start_sample: 0,
                samples: vec![0.25; SECTION].into(),
                right: Some(vec![-0.125; SECTION].into()),
            }])
            .unwrap();
        control.replace(&case).unwrap();
        control.preview_fader(leaf.raw(), 2.0, 0.0).unwrap();
        // A stopped preview settles before play, so there is no initial ramp.
        let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
        audio.render(&mut left, &mut right);
        control.service();
        control.command(PlaybackCommand::Play).unwrap();
        audio.render(&mut left, &mut right);
        assert!(left.iter().all(|sample| *sample == expected));
        assert!(right.iter().all(|sample| *sample == -expected / 2.0));
        assert!(!control.status().failed);
    }
}
