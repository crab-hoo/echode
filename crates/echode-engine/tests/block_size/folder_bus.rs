//! A child's audio goes through its parent's fader (ADR 0277, ADR 0338).
//!
//! The fixture is the one in `folder_mix`: root{inner{leaf}, peer}, outside.
//! Every clip is constant, left = level and right = -level / 2.
use std::sync::Arc;

use echode_engine::{
    CachedTrack, PlaybackCommand, PlaybackOptions, playback, render_offline_blocks_with_clips,
};
use echode_model::ParamKey;

use super::BLOCKS;
use super::folder_mix::{LEVELS, SECTION, State, TREE, clips, fixture, ramped, set_value};

fn peak(control: &echode_engine::PlaybackControl, track: u128) -> f32 {
    control
        .meters()
        .into_iter()
        .find(|meter| meter.track == track)
        .unwrap()
        .peak()
}

/// FCA-06: before buses, a folder's gain and pan changed only its own clip.
#[test]
fn folder_faders_reach_their_children_and_match_offline_audio_under_rtsan() {
    let (project, ids @ [root, inner, leaf, peer, outside]) = fixture();
    // Root: gain 1/2, balance 1/2 to the right, so left × 1/4 and right × 1/2.
    // Inner: gain 1/4 at center. Powers of two keep every sum exact.
    let case = set_value(&project, root, ParamKey::Gain, 0.5);
    let case = set_value(&case, root, ParamKey::Pan, 0.5);
    let case = set_value(&case, inner, ParamKey::Gain, 0.25);
    let [l_root, l_inner, l_leaf, l_peer, l_outside] = LEVELS;
    let inner_bus = (l_inner + l_leaf) * 0.25;
    let root_bus = l_root + l_peer + inner_bus;
    let left = root_bus * 0.25 + l_outside;
    let right = -(root_bus / 2.0) * 0.5 - l_outside / 2.0;
    let loaded = clips(ids, SECTION);
    for rate in [48_000, 96_000] {
        for block in BLOCKS {
            let (mut control, mut audio) =
                playback(&case, rate, block as usize, PlaybackOptions::default()).unwrap();
            let live = loaded.clone();
            control.set_clip_loader(Arc::new(move |_, _| Ok(live.clone())));
            control.replace(&case).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let (mut l, mut r) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            let mut heard = Vec::new();
            for offset in (0..SECTION).step_by(block as usize) {
                let count = (block as usize).min(SECTION - offset);
                audio.render(&mut l[..count], &mut r[..count]);
                for (&sample_left, &sample_right) in l.iter().zip(&r).take(count) {
                    assert_eq!(
                        (sample_left, sample_right),
                        (left, right),
                        "rate {rate}, block {block}"
                    );
                    heard.extend_from_slice(&[sample_left, sample_right]);
                }
            }
            // A parent's meter is its fader's output, children included.
            assert_eq!(peak(&control, root.raw()), root_bus * 0.25);
            assert_eq!(peak(&control, inner.raw()), inner_bus);
            assert_eq!(peak(&control, leaf.raw()), l_leaf);
            assert_eq!(peak(&control, peer.raw()), l_peer);
            assert_eq!(peak(&control, outside.raw()), l_outside);
            let mut offline = Vec::new();
            render_offline_blocks_with_clips(
                &case,
                rate,
                SECTION as u32,
                block,
                &loaded,
                |samples| {
                    offline.extend_from_slice(samples);
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
            assert_eq!(heard, offline, "rate {rate}, block {block}");
            assert!(!control.status().failed);
        }
    }
}

/// ADR 0244 kept: soloing only a child does not play the parent's own audio,
/// but the child still goes through the parent's fader.
#[test]
fn a_parent_passes_a_soloed_child_but_not_its_own_audio_under_rtsan() {
    let (project, ids @ [root, inner, leaf, peer, outside]) = fixture();
    let quiet = set_value(&project, root, ParamKey::Gain, 0.5);
    let soloed = set_value(&quiet, leaf, ParamKey::Solo, 1.0);
    let gains = [0.5, 1.0, 1.0, 1.0, 1.0];
    let all = State {
        parents: TREE,
        gains,
        open: [true; 5],
        own: [true; 5],
    };
    let solo = State {
        parents: TREE,
        gains,
        // Root and inner pass the leaf; peer and outside are outside the solo.
        open: [true, true, true, false, false],
        // Root and inner keep their faders open but gate their own clips.
        own: [false, false, true, true, true],
    };
    // Meters hold and release, so read them from a play that starts soloed.
    let (mut control, mut audio) =
        playback(&soloed, 48_000, 64, PlaybackOptions::default()).unwrap();
    let steady = clips(ids, SECTION);
    control.set_clip_loader(Arc::new(move |_, _| Ok(steady.clone())));
    control.replace(&soloed).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut l, &mut r);
    let [_, _, l_leaf, _, _] = LEVELS;
    assert_eq!(peak(&control, leaf.raw()), l_leaf);
    assert_eq!(
        peak(&control, inner.raw()),
        l_leaf,
        "inner's own clip leaked"
    );
    assert_eq!(peak(&control, root.raw()), l_leaf * 0.5);
    assert_eq!(peak(&control, peer.raw()), 0.0);
    assert_eq!(peak(&control, outside.raw()), 0.0);
    // Moving the soloed leaf to the top level leaves inner without children
    // while its own gate is closed. The gate still ramps open under its
    // closing fader instead of letting inner's clip in at once.
    let detached = soloed.set_parent(leaf, None).unwrap();
    let alone = State {
        parents: [None, Some(0), None, Some(0), None],
        gains,
        open: [false, false, true, false, false],
        own: [true; 5],
    };
    let cases = [
        (quiet.clone(), all),
        (soloed.clone(), solo),
        (detached, alone),
        (soloed, solo),
        (quiet, all),
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
            let live = loaded.clone();
            control.set_clip_loader(Arc::new(move |_, _| Ok(live.clone())));
            control.replace(&cases[0].0).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let (mut l, mut r) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            let mut heard = Vec::new();
            let mut previous = cases[0].1;
            for (index, (case, target)) in cases.iter().enumerate() {
                if index != 0 {
                    control.replace(case).unwrap();
                }
                for offset in (0..SECTION).step_by(block as usize) {
                    let count = (block as usize).min(SECTION - offset);
                    audio.render(&mut l[..count], &mut r[..count]);
                    control.service();
                    for (frame, (&left, &right)) in l.iter().zip(&r).take(count).enumerate() {
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

/// A cache stands for one track's device output. A parent's output depends on
/// its children's bus, so a cache given for a parent is not read.
#[test]
fn a_cache_given_for_a_parent_is_not_read_under_rtsan() {
    let (project, ids @ [root, ..]) = fixture();
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    let loaded = clips(ids, SECTION);
    control.set_clip_loader(Arc::new(move |_, _| Ok(loaded.clone())));
    control
        .set_cached_tracks(&[CachedTrack {
            track: root.raw(),
            sample_rate: 48_000,
            start_sample: 0,
            samples: vec![0.75; SECTION].into(),
            right: None,
        }])
        .unwrap();
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = ([0.0; 64], [0.0; 64]);
    audio.render(&mut left, &mut right);
    let all: f32 = LEVELS.into_iter().sum();
    assert!(left.iter().all(|sample| *sample == all), "{left:?}");
    assert!(right.iter().all(|sample| *sample == -all / 2.0));
    assert!(!control.status().failed);
}
