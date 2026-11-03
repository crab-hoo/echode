use super::*;
use echode_engine::{LoadedClip, PlaybackCommand, PlaybackOptions, playback};

fn render(
    project: &Project,
    rate: u32,
    block: usize,
    clips: Vec<LoadedClip>,
    looping: bool,
) -> Vec<(f32, f32)> {
    let (mut control, mut audio) =
        playback(project, rate, block, PlaybackOptions::default()).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
    if looping {
        control
            .command(PlaybackCommand::SetLoop {
                start: 113,
                end: 1007,
            })
            .unwrap();
    } else {
        control.replace(project).unwrap();
    }
    control.command(PlaybackCommand::Play).unwrap();
    // These durations and buffers exercise this regression, not playback limits.
    let total = 2737;
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    let mut heard = Vec::new();
    for offset in (0..total).step_by(block) {
        let count = block.min(total - offset);
        audio.render(&mut left[..count], &mut right[..count]);
        heard.extend(
            left[..count]
                .iter()
                .copied()
                .zip(right[..count].iter().copied()),
        );
        control.service();
    }
    assert!(!control.status().failed);
    heard
}

fn assert_samples(actual: &[(f32, f32)], expected: &[(f32, f32)], context: &str) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual, expected, "{context}, frame {index}");
    }
}

#[test]
fn unscaled_clip_gain_and_mono_promotion_match_baked_pcm_under_rtsan() {
    let (project, track) = Project::new().add_track("unscaled clips");
    for rate in [48_000, 96_000] {
        for gain in [0.0, 0.37, 2.3, echode_model::CLIP_GAIN_MAX] {
            for stereo in [false, true] {
                let clips: Vec<_> = [257, 343, 257]
                    .into_iter()
                    .scan(173, |start, frames| {
                        let at = *start;
                        *start += frames;
                        Some((at, frames))
                    })
                    .enumerate()
                    .map(|(index, (start, frames))| {
                        let samples: Arc<[f32]> = (0..frames)
                            .map(|i| ((i as f32 * 0.031) + 0.7).sin() * 0.35)
                            .collect();
                        LoadedClip {
                            track: track.raw(),
                            start_sample: start,
                            right: (stereo && index == 1)
                                .then(|| samples.iter().map(|sample| -sample * 0.53).collect()),
                            samples,
                            gain,
                            fade_in: 127,
                            fade_out: 109,
                        }
                    })
                    .collect();
                // Reference the previous multiplication order, including f32
                // rounding before the mono plane is copied to both channels.
                let baked: Vec<_> = clips
                    .iter()
                    .map(|clip| {
                        let promotion = if stereo && clip.right.is_none() {
                            std::f32::consts::FRAC_1_SQRT_2
                        } else {
                            1.0
                        };
                        let left: Arc<[f32]> = clip
                            .samples
                            .iter()
                            .map(|sample| sample * gain * promotion)
                            .collect();
                        let right = clip
                            .right
                            .as_ref()
                            .map(|plane| plane.iter().map(|sample| sample * gain).collect());
                        LoadedClip {
                            samples: Arc::clone(&left),
                            right: right.or_else(|| stereo.then(|| Arc::clone(&left))),
                            gain: 1.0,
                            ..clip.clone()
                        }
                    })
                    .collect();
                for looping in [false, true] {
                    let mut reference: Option<Vec<(f32, f32)>> = None;
                    for block in BLOCKS {
                        let heard = render(&project, rate, block as usize, clips.clone(), looping);
                        let context = format!(
                            "rate {rate}, gain {gain}, stereo {stereo}, loop {looping}, block {block}"
                        );
                        assert_samples(
                            &heard,
                            &render(&project, rate, block as usize, baked.clone(), looping),
                            &context,
                        );
                        if let Some(reference) = &reference {
                            assert_samples(&heard, reference, &context);
                        } else {
                            reference = Some(heard);
                        }
                    }
                    let heard = reference.unwrap();
                    if gain == 0.0 {
                        assert!(
                            heard
                                .iter()
                                .all(|&(left, right)| left == 0.0 && right == 0.0)
                        );
                    } else {
                        assert!(heard.iter().any(|&(left, _)| left.abs() > gain * 0.001));
                    }
                    if !looping {
                        let mut exported = Vec::new();
                        echode_engine::render_offline_blocks_with_clips(
                            &project,
                            rate,
                            heard.len() as u32,
                            511,
                            &clips,
                            |block| {
                                exported
                                    .extend(block.chunks_exact(2).map(|pair| (pair[0], pair[1])));
                                Ok::<_, ()>(())
                            },
                        )
                        .unwrap();
                        assert_samples(&heard, &exported, "offline export");
                    }
                }
            }
        }
    }
}
