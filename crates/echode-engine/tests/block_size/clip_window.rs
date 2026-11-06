//! A placement that reads a window of shared source planes (EXT-01) must sound
//! exactly like the copied window it replaced, for every block length.

use super::*;
use echode_engine::{ClipWindow, LoadedClip};

/// A copy of what `window` plays: zeros outside the source, as the loader
/// used to store it.
fn copied(source: &[f32], window: ClipWindow) -> Arc<[f32]> {
    (0..window.frames)
        .map(|index| {
            usize::try_from(window.lead + index as i64)
                .ok()
                .and_then(|at| source.get(at).copied())
                .unwrap_or(0.0)
        })
        .collect()
}

#[test]
fn windowed_clips_match_copied_windows_for_every_block_size_under_rtsan() {
    let (project, track) = Project::new().add_track("windows");
    // The source and window lengths cover a trim inside the source, a lead
    // before it, a tail past it, and a window wider than the source on both
    // sides. They are this regression's sizes, not playback limits.
    let left: Arc<[f32]> = (0..601)
        .map(|i| ((i as f32 * 0.043) + 0.2).sin() * 0.4)
        .collect();
    let right: Arc<[f32]> = left.iter().map(|sample| -sample * 0.61).collect();
    let windows = [
        ClipWindow {
            lead: 97,
            frames: 311,
        },
        ClipWindow {
            lead: -83,
            frames: 257,
        },
        ClipWindow {
            lead: 455,
            frames: 299,
        },
        ClipWindow {
            lead: -41,
            frames: 700,
        },
    ];
    for stereo in [false, true] {
        let mut start = 131;
        let mut shared = Vec::new();
        let mut copies = Vec::new();
        for window in windows {
            let clip = LoadedClip {
                track: track.raw(),
                start_sample: start,
                samples: Arc::clone(&left),
                right: stereo.then(|| Arc::clone(&right)),
                window: Some(window),
                gain: 0.83,
                fade_in: 61,
                fade_out: 53,
            };
            for index in 0..window.frames {
                assert_eq!(clip.left_at(index), copied(&left, window)[index]);
            }
            copies.push(LoadedClip {
                samples: copied(&left, window),
                right: stereo.then(|| copied(&right, window)),
                window: None,
                ..clip.clone()
            });
            shared.push(clip);
            start += i64::try_from(window.frames).unwrap() + 19;
        }
        for looping in [false, true] {
            let mut reference: Option<Vec<(f32, f32)>> = None;
            for block in BLOCKS {
                let context = format!("stereo {stereo}, loop {looping}, block {block}");
                let heard =
                    clip_gain::render(&project, RATE, block as usize, shared.clone(), looping);
                clip_gain::assert_samples(
                    &heard,
                    &clip_gain::render(&project, RATE, block as usize, copies.clone(), looping),
                    &context,
                );
                if let Some(reference) = &reference {
                    clip_gain::assert_samples(&heard, reference, &context);
                } else {
                    reference = Some(heard);
                }
            }
            let heard = reference.unwrap();
            assert!(heard.iter().any(|&(left, _)| left.abs() > 0.1));
        }
    }
}
