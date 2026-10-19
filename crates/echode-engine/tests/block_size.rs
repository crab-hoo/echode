//! Output must not depend on how the timeline is cut into blocks.
//!
//! Offline export uses one block length, a device uses another, and the preview
//! cache must match live playback (ARCHITECTURE §7.8). Each test renders the
//! same material with several block lengths and compares the samples bit for bit.
//! The block lengths and voice slots are this test's choice, not engine limits.

use echode_engine::{
    AudioClip, Event, PlayedSpan, SynthVoice, TrackFader, Transport, fade_loop_seam, mix_tracks,
    render_audio_clips, render_metronome, render_test_synth, schedule, seam_fade_frames,
};
use echode_model::{
    AudioRef, Id, Meter, NoteDraft, Project, TICKS_PER_QUARTER, Tempo, TempoMap, Tick,
};
use std::sync::Arc;

const RATE: u32 = 48_000;
const BLOCKS: [u32; 4] = [1, 7, 64, 512];
const VOICES_PER_TRACK: usize = 8;

#[test]
fn bypass_copies_each_plane_without_partition_or_gain_changes() {
    // Three planes and this duration exercise this copy operation only; neither
    // is a device/channel limit. Product buses validate their own layouts.
    let input: Vec<Vec<f32>> = (0..3)
        .map(|channel| {
            (0..1537)
                .map(|n| (n as f32 * 0.031 + channel as f32).sin())
                .collect()
        })
        .collect();
    for block in BLOCKS {
        let mut output = vec![vec![0.0; block as usize]; input.len()];
        let mut heard = vec![Vec::new(); input.len()];
        for offset in (0..input[0].len()).step_by(block as usize) {
            let frames = (block as usize).min(input[0].len() - offset);
            echode_engine::copy_bypassed_audio(
                input.iter().map(|plane| &plane[offset..offset + frames]),
                &mut output,
                frames,
            );
            for (heard, output) in heard.iter_mut().zip(&output) {
                heard.extend_from_slice(&output[..frames]);
            }
        }
        assert_eq!(heard, input);
        echode_engine::copy_bypassed_audio(std::iter::empty(), &mut output, block as usize);
        assert!(output.iter().flatten().all(|sample| *sample == 0.0));
    }
}

#[test]
fn contract_pre_fader_cached_track_bypasses_devices_under_rtsan() {
    use echode_engine::{CachedTrack, PlaybackCommand, playback_pair};
    use echode_model::ParamKey;
    let (live, track) = Project::new().add_track("source port");
    let live = live.insert_note(track, note(0.0, 4.0, 60)).unwrap().0;
    let gain = live.tracks()[0].devices[0]
        .parameters
        .iter()
        .find(|parameter| parameter.key == ParamKey::Gain)
        .unwrap()
        .id;
    let live = live.set_parameter(gain, 0.5).unwrap();
    let project = live
        .add_clap_device(track, Id::from_raw(1), "missing.fixture.instrument")
        .unwrap();
    for rate in [48_000, 96_000] {
        let mut reference = None;
        for block in BLOCKS {
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            control.set_routes(std::path::Path::new("/missing/fixture/helper"), &[]);
            // These planes cover this fixture only; a provider can prepare any
            // number of tracks or any duration outside the callback.
            let cache = CachedTrack {
                track: track.raw(),
                sample_rate: rate,
                start_sample: 137,
                samples: vec![0.25; 3000].into(),
                right: Some(vec![-0.5; 3000].into()),
            };
            control
                .set_cached_tracks(std::slice::from_ref(&cache))
                .unwrap();
            let invalid = CachedTrack {
                sample_rate: rate / 2,
                ..cache.clone()
            };
            assert!(control.set_cached_tracks(&[invalid]).is_err());
            assert!(
                control.replace(&project).is_ok(),
                "cached source tried to open its live devices"
            );
            control
                .command(PlaybackCommand::SetLoop {
                    start: 137,
                    end: 1146,
                })
                .unwrap();
            let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            for _ in 0..4 {
                audio.render(&mut left, &mut right);
                control.service();
            }
            assert!(!control.status().pending_plan);
            control.command(PlaybackCommand::Play).unwrap();
            assert!(
                control.set_cached_tracks(&[]).is_err(),
                "queued Play must also exclude source reconfiguration"
            );
            let mut output = Vec::new();
            for offset in (0..2300).step_by(block as usize) {
                let n = (block as usize).min(2300 - offset);
                audio.render(&mut left[..n], &mut right[..n]);
                assert!(!control.status().failed);
                output.extend(left[..n].iter().copied().zip(right[..n].iter().copied()));
                control.service();
            }
            assert!(output[..137].iter().all(|sample| *sample == (0.0, 0.0)));
            assert!(output[137..].iter().all(|sample| *sample == (0.125, -0.25)));
            if let Some(reference) = &reference {
                assert_eq!(&output, reference);
            } else {
                reference = Some(output);
            }
            // The same cached device output still passes through a changed fader.
            control
                .replace(&project.set_parameter(gain, 0.25).unwrap())
                .unwrap();
            for _ in 0..(rate as usize / 100 / block as usize + 4) {
                audio.render(&mut left, &mut right);
                control.service();
            }
            assert!(left.iter().all(|sample| *sample == 0.0625));
            assert!(right.iter().all(|sample| *sample == -0.125));
            control.command(PlaybackCommand::Pause).unwrap();
            audio.render(&mut left, &mut right);
            control.service();
            control.set_cached_tracks(&[]).unwrap();
            control.set_routes(std::path::Path::new(""), &[]);
            control.replace(&live).unwrap();
            for _ in 0..4 {
                audio.render(&mut left, &mut right);
                control.service();
            }
            control.command(PlaybackCommand::Play).unwrap();
            for _ in 0..4 {
                audio.render(&mut left, &mut right);
            }
            assert!(left.iter().any(|sample| *sample != 0.0));
            assert!(left.iter().any(|sample| *sample != 0.125));
        }
    }
}

#[test]
fn live_fader_preview_switches_only_previous_and_next_track_under_rtsan() {
    use echode_engine::{PlaybackCommand, playback_pair};
    let (project, a) = Project::new().add_track("first");
    let (project, b) = project.add_track("second");
    let (project, c) = project.add_track("unrelated");
    let tracks = [(a, 0.125), (b, 0.25), (c, 0.5)];
    for rate in [48_000, 96_000] {
        let mut reference = None;
        for block in BLOCKS {
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            // These planes cover only this four-phase fixture's timeline.
            let clips: Vec<_> = tracks
                .iter()
                .map(|&(track, level)| stereo_clip(track, vec![level; 6000], vec![level; 6000]))
                .collect();
            control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
            control.replace(&project).unwrap();
            let mut output = Vec::new();
            let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            for phase in 0..4 {
                let expected = if let Some(&(track, level)) = tracks.get(phase) {
                    control.preview_fader(track.raw(), 0.0, 0.0).unwrap();
                    0.875 - level
                } else {
                    control.clear_fader_preview(false).unwrap();
                    0.875
                };
                if phase == 0 {
                    control.command(PlaybackCommand::Play).unwrap();
                }
                let end = (phase as i64 + 1) * 1000;
                while control.status().sample < end {
                    let n = (block as usize).min((end - control.status().sample) as usize);
                    audio.render_live_gated_until(
                        &mut left[..n],
                        &mut right[..n],
                        std::time::Instant::now() + std::time::Duration::from_secs(5),
                        true,
                    );
                    assert!(!control.status().failed);
                    output.extend(left[..n].iter().copied().zip(right[..n].iter().copied()));
                    control.service();
                }
                assert_eq!(output.last(), Some(&(expected, expected)));
            }
            if let Some(reference) = &reference {
                assert_eq!(&output, reference);
            } else {
                reference = Some(output);
            }
        }
    }
}

#[test]
fn contract_live_fader_preview_and_peak_taps_under_rtsan() {
    use echode_engine::{PlaybackAudio, PlaybackCommand, PlaybackControl, playback_pair};
    use echode_model::ParamKey;
    fn until(
        audio: &mut PlaybackAudio,
        control: &mut PlaybackControl,
        end: i64,
        block: usize,
        output: &mut Vec<(f32, f32)>,
    ) {
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        while control.status().sample < end {
            let n = block.min((end - control.status().sample) as usize);
            audio.render_live_gated_until(
                &mut left[..n],
                &mut right[..n],
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                true,
            );
            assert!(!control.status().failed);
            output.extend(left[..n].iter().copied().zip(right[..n].iter().copied()));
            control.service();
        }
    }
    for rate in [48_000, 96_000] {
        let mut reference = None;
        for block in BLOCKS {
            let (project, track) = Project::new().add_track("meter and preview");
            let parameter = |key| {
                project.tracks()[0].devices[0]
                    .parameters
                    .iter()
                    .find(|p| p.key == key)
                    .unwrap()
                    .id
            };
            let committed = project
                .set_parameter(parameter(ParamKey::Gain), 0.5)
                .unwrap()
                .set_parameter(parameter(ParamKey::Pan), 1.0)
                .unwrap();
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            // These samples cover this fixture's timeline only.
            let clip = stereo_clip(track, vec![0.25; 10_000], vec![-0.5; 10_000]);
            control.set_clip_loader(Arc::new(move |_, _| Ok(vec![clip.clone()])));
            control.replace(&project).unwrap();
            let meter = control.meters().pop().unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = Vec::new();
            until(&mut audio, &mut control, 137, block as usize, &mut output);
            assert_eq!(meter.peak(), 0.5);
            control.preview_fader(track.raw(), 0.5, 1.0).unwrap();
            until(&mut audio, &mut control, 800, block as usize, &mut output);
            assert_eq!(output.last(), Some(&(0.0, -0.25)));
            assert_eq!(control.status().plugin_missed_blocks, 0);

            // The clear must wait for the committed bank, even with an older
            // bank still queued. It must never briefly restore the old gain.
            control.replace(&project).unwrap();
            control.replace(&committed).unwrap();
            control.clear_fader_preview(true).unwrap();
            until(&mut audio, &mut control, 1600, block as usize, &mut output);
            assert!(
                output[800..1600]
                    .iter()
                    .all(|sample| *sample == (0.0, -0.25))
            );
            assert!(Arc::ptr_eq(&meter, &control.meters()[0]));

            control.preview_fader(track.raw(), 0.0, 0.0).unwrap();
            until(&mut audio, &mut control, 2300, block as usize, &mut output);
            assert_eq!(output.last(), Some(&(0.0, 0.0)));
            control.clear_fader_preview(false).unwrap();
            until(&mut audio, &mut control, 3000, block as usize, &mut output);
            assert_eq!(output.last(), Some(&(0.0, -0.25)));
            assert!(!control.status().pending_plan);
            if let Some(reference) = &reference {
                assert_eq!(&output, reference);
            } else {
                reference = Some(output);
            }

            let before = meter.peak();
            control.command(PlaybackCommand::Pause).unwrap();
            let mut left = vec![0.0; block as usize];
            let mut right = left.clone();
            for _ in 0..(rate / block + 1) {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                    true,
                );
            }
            assert!(meter.peak() < before / 8.0, "paused meters did not decay");
            // Two more seconds cross the shared -60 dB floor from that level.
            // These renders only cover the meter release, not a longer timeline.
            for _ in 0..(2 * rate / block + 1) {
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                    true,
                );
            }
            assert_eq!(meter.peak(), 0.0, "silence kept a runaway peak");
            assert_eq!(control.status().sample, 3000);
            assert!(control.preview_fader(track.raw(), f32::NAN, 0.0).is_err());
        }
    }
}

#[test]
fn ordinary_silent_to_note_boundaries_leave_the_source_unchanged() {
    for rate in [48_000, 96_000] {
        let mut source = vec![0.0f32; 1003];
        // Include a gentle attack and a deliberate sharp attack. Neither is
        // a transport discontinuity, even when it lands at a block boundary.
        for (age, sample) in source[137..337].iter_mut().enumerate() {
            *sample = 0.25 * (age.min(32) as f32 / 32.0);
        }
        source[700..900].fill(-0.5);
        let expected_right: Vec<_> = source.iter().map(|sample| -*sample / 2.0).collect();
        for block in BLOCKS {
            let mut left = source.clone();
            let mut right = expected_right.clone();
            let mut declick = echode_engine::OutputDeclick::new(rate);
            for (index, (left, right)) in left
                .chunks_mut(block as usize)
                .zip(right.chunks_mut(block as usize))
                .enumerate()
            {
                declick.process(left, right, index == 0);
            }
            assert_eq!(left, source);
            assert_eq!(right, expected_right);
        }
    }
}

fn clip_player(
    project: &Project,
    block: usize,
    clips: Vec<echode_engine::LoadedClip>,
) -> (echode_engine::PlaybackControl, echode_engine::PlaybackAudio) {
    let (mut control, audio) = echode_engine::playback_pair(project, RATE, block).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
    control.replace(project).unwrap();
    control
        .command(echode_engine::PlaybackCommand::Play)
        .unwrap();
    (control, audio)
}

fn stereo_clip(track: Id, left: Vec<f32>, right: Vec<f32>) -> echode_engine::LoadedClip {
    echode_engine::LoadedClip {
        track: track.raw(),
        start_sample: 0,
        samples: left.into(),
        right: Some(right.into()),
        fade_in: 0,
        fade_out: 0,
    }
}

#[test]
fn stereo_playback_and_export_preserve_opposite_phase_for_every_block_size() {
    let (project, track) = Project::new().add_track("stereo");
    let frames = 1003;
    let left: Vec<_> = (0..frames).map(|index| index as f32 / 4096.0).collect();
    let right: Vec<_> = left.iter().map(|sample| -*sample).collect();
    let clips = vec![stereo_clip(track, left.clone(), right.clone())];
    let expected: Vec<_> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    for block in BLOCKS {
        let (control, mut audio) = clip_player(&project, block as usize, clips.clone());
        let mut output = Vec::new();
        for offset in (0..frames).step_by(block as usize) {
            let count = (frames - offset).min(block as usize);
            let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
            audio.render(&mut left, &mut right);
            output.extend(left.into_iter().zip(right).flat_map(|(l, r)| [l, r]));
        }
        assert!(!control.status().failed);
        assert_eq!(output, expected, "live block {block}");
        let mut exported = Vec::new();
        echode_engine::render_offline_blocks_with_clips(
            &project,
            RATE,
            frames as u32,
            block,
            &clips,
            |block| {
                exported.extend_from_slice(block);
                Ok::<_, ()>(())
            },
        )
        .unwrap();
        assert_eq!(exported, expected, "export block {block}");
    }
}

#[test]
fn product_audio_loop_fades_follow_project_positions_across_blocks() {
    let (project, track) = Project::new().add_track("loop");
    let start = 337;
    let end = 1360;
    let fade = seam_fade_frames(RATE) as usize;
    let clips = vec![stereo_clip(track, vec![0.25; 2048], vec![-0.5; 2048])];
    let mut reference = None;
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        let clips = clips.clone();
        control.set_clip_loader(Arc::new(move |_, _| Ok(clips.clone())));
        control
            .command(echode_engine::PlaybackCommand::SetLoop { start, end })
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::Locate(start))
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let frames = (end - start) as usize * 3;
        let mut output = Vec::new();
        for offset in (0..frames).step_by(block as usize) {
            let count = (frames - offset).min(block as usize);
            let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
            audio.render(&mut left, &mut right);
            for (l, r) in left.into_iter().zip(right) {
                assert_eq!(r, -2.0 * l);
                output.push(l);
            }
        }
        assert!(!control.status().failed);
        for pass in output.chunks_exact((end - start) as usize) {
            assert_eq!(pass[0], 0.0);
            assert_eq!(pass[pass.len() - 1], 0.0);
            assert_eq!(pass[fade], 0.25);
            assert!(
                pass.windows(2)
                    .all(|pair| (pair[1] - pair[0]).abs() <= 0.25 / (fade - 1) as f32 + 1.0e-7)
            );
        }
        if let Some(expected) = &reference {
            assert_eq!(&output, expected, "loop block {block}");
        } else {
            reference = Some(output);
        }
    }
}

#[test]
fn fader_edits_ramp_without_restarting_on_unrelated_replacement() {
    use echode_model::ParamKey;
    let (project, track) = Project::new().add_track("fader");
    let clips = vec![stereo_clip(track, vec![0.25; 2048], vec![-0.25; 2048])];
    let parameter = |key| {
        project.tracks()[0].devices[0]
            .parameters
            .iter()
            .find(|p| p.key == key)
            .unwrap()
            .id
    };
    let muted = project
        .set_parameter(parameter(ParamKey::Mute), 1.0)
        .unwrap();
    let panned = project
        .set_parameter(parameter(ParamKey::Pan), -1.0)
        .unwrap();
    let quiet = panned
        .set_parameter(parameter(ParamKey::Gain), 0.5)
        .unwrap();
    let edits = [
        (64, &muted),
        (101, &muted),
        (400, &panned),
        (480, &quiet),
        (503, &quiet),
    ];
    let mut reference = None;
    for block in BLOCKS {
        let (mut control, mut audio) = clip_player(&project, block as usize, clips.clone());
        let mut output = Vec::new();
        let mut position = 0;
        let mut edit = 0;
        while position < 900 {
            if let Some((at, project)) = edits.get(edit)
                && position == *at
            {
                control.replace(project).unwrap();
                edit += 1;
            }
            let next = edits.get(edit).map_or(900, |(at, _)| *at);
            let count = (next - position).min(block as usize);
            let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
            audio.render(&mut left, &mut right);
            output.extend(left.into_iter().zip(right));
            position += count;
        }
        assert!(!control.status().failed);
        assert_eq!(output[63], (0.25, -0.25));
        assert!(output[64].0 < 0.25 && output[64].0 > 0.248);
        assert_eq!(output[183], (0.125, -0.125));
        assert_eq!(output[303], (0.0, 0.0));
        assert_eq!(output[899], (0.125, 0.0));
        assert!(
            output
                .windows(2)
                .all(|pair| (pair[1].0 - pair[0].0).abs() < 0.002)
        );
        if let Some(expected) = &reference {
            assert_eq!(&output, expected, "fader block {block}");
        } else {
            reference = Some(output);
        }
    }
}

#[test]
fn product_master_accumulates_tracks_in_double_precision() {
    let mut project = Project::new();
    let mut clips = Vec::new();
    for value in [16_777_216.0, 1.0, -16_777_216.0] {
        let (next, track) = project.add_track("sum");
        project = next;
        clips.push(stereo_clip(track, vec![value; 1], vec![-value; 1]));
    }
    let (control, mut audio) = clip_player(&project, 1, clips);
    let (mut left, mut right) = ([0.0], [0.0]);
    audio.render(&mut left, &mut right);
    assert!(!control.status().failed);
    assert_eq!(left, [1.0]);
    assert_eq!(right, [-1.0]);
}

#[test]
fn short_audio_loops_keep_an_interior_and_never_depend_on_blocks() {
    let (project, track) = Project::new().add_track("short");
    for length in 1..=8 {
        let mut reference = None;
        for block in BLOCKS {
            let (mut control, mut audio) =
                echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![stereo_clip(track, vec![0.5; 32], vec![-0.5; 32])])
            }));
            control
                .command(echode_engine::PlaybackCommand::SetLoop {
                    start: 7,
                    end: 7 + length,
                })
                .unwrap();
            control
                .command(echode_engine::PlaybackCommand::Locate(7))
                .unwrap();
            control
                .command(echode_engine::PlaybackCommand::Play)
                .unwrap();
            let frames = length as usize * 4;
            let mut heard = Vec::new();
            for offset in (0..frames).step_by(block as usize) {
                let count = (frames - offset).min(block as usize);
                let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
                audio.render(&mut left, &mut right);
                assert!(
                    left.iter()
                        .zip(&right)
                        .all(|(l, r)| l.is_finite() && *r == -*l)
                );
                heard.extend(left);
            }
            assert!(!control.status().failed);
            assert!(
                heard.iter().any(|sample| *sample > 0.0),
                "silenced {length}-frame loop"
            );
            if let Some(expected) = &reference {
                assert_eq!(&heard, expected);
            } else {
                reference = Some(heard);
            }
        }
    }
}

#[test]
fn mono_and_stereo_placements_keep_their_bus_gains_on_the_same_track() {
    let (project, track) = Project::new().add_track("mixed");
    let mono = echode_engine::LoadedClip {
        track: track.raw(),
        start_sample: 0,
        samples: Arc::from(vec![0.5; 8]),
        right: None,
        fade_in: 0,
        fade_out: 0,
    };
    let mut stereo = stereo_clip(track, vec![0.5; 8], vec![-0.25; 8]);
    stereo.start_sample = 8;
    for block in BLOCKS {
        let (control, mut audio) =
            clip_player(&project, block as usize, vec![mono.clone(), stereo.clone()]);
        let mut heard = Vec::new();
        for offset in (0..16).step_by(block as usize) {
            let count = (16 - offset).min(block as usize);
            let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
            audio.render(&mut left, &mut right);
            heard.extend(left.into_iter().zip(right));
        }
        assert!(!control.status().failed);
        assert_eq!(
            heard[..8],
            [(
                0.5 * std::f32::consts::FRAC_1_SQRT_2,
                0.5 * std::f32::consts::FRAC_1_SQRT_2
            ); 8]
        );
        assert_eq!(heard[8..], [(0.5, -0.25); 8]);
    }
}

#[test]
fn solo_and_mute_changes_ramp_with_mute_priority() {
    use echode_model::ParamKey;
    let (project, a) = Project::new().add_track("a");
    let (project, b) = project.add_track("b");
    let parameter = |key| {
        project.tracks()[1].devices[0]
            .parameters
            .iter()
            .find(|p| p.key == key)
            .unwrap()
            .id
    };
    let soloed = project
        .set_parameter(parameter(ParamKey::Solo), 1.0)
        .unwrap();
    let muted = soloed
        .set_parameter(parameter(ParamKey::Mute), 1.0)
        .unwrap();
    let unsoloed = muted.set_parameter(parameter(ParamKey::Solo), 0.0).unwrap();
    let clips = vec![
        stereo_clip(a, vec![0.25; 1024], vec![0.25; 1024]),
        stereo_clip(b, vec![0.5; 1024], vec![0.5; 1024]),
    ];
    let mut reference = None;
    for block in BLOCKS {
        let (mut control, mut audio) = clip_player(&project, block as usize, clips.clone());
        let mut heard = Vec::new();
        for (index, (project, frames, expected)) in [
            (&project, 64, 0.75),
            (&soloed, 240, 0.5),
            (&muted, 240, 0.0),
            (&unsoloed, 240, 0.25),
        ]
        .iter()
        .enumerate()
        {
            if index > 0 {
                control.replace(project).unwrap();
            }
            let mut remaining = *frames;
            while remaining > 0 {
                let count = remaining.min(block as usize);
                let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
                audio.render(&mut left, &mut right);
                assert_eq!(left, right);
                heard.extend(left);
                remaining -= count;
            }
            assert_eq!(heard.last(), Some(expected));
        }
        assert!(!control.status().failed);
        assert!(
            heard
                .windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() <= 0.5 / 240.0 + 1.0e-7)
        );
        if let Some(expected) = &reference {
            assert_eq!(&heard, expected);
        } else {
            reference = Some(heard);
        }
    }
}

#[test]
fn cancelled_output_drains_edits_without_advancing_for_every_block_size() {
    use echode_engine::PlaybackCommand;
    use std::time::{Duration, Instant};
    let (project, track) = Project::new().add_track("cancelled transport");
    let mute = project.tracks()[0].devices[0]
        .parameters
        .iter()
        .find(|parameter| parameter.key == echode_model::ParamKey::Mute)
        .unwrap()
        .id;
    for block in BLOCKS {
        let (mut control, mut audio) = clip_player(
            &project,
            block as usize,
            vec![stereo_clip(track, vec![0.25; 2048], vec![-0.5; 2048])],
        );
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        audio.render(&mut left, &mut right);
        let held = control.status().sample;
        // More edits than the ownership rings can hold at once. Idle callbacks
        // must continue consuming plans, without moving or leaking any sound.
        for edit in 0..32 {
            let changed = project
                .set_parameter(mute, if edit % 2 == 0 { 0.0 } else { 1.0 })
                .unwrap();
            control.replace(&changed).unwrap();
            audio.render_gated_until(
                &mut left,
                &mut right,
                Instant::now() + Duration::from_secs(1),
                false,
            );
            assert_eq!(control.status().sample, held);
            assert!(!control.status().failed && !control.status().playing);
            assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
            control.service();
        }
        control.command(PlaybackCommand::Play).unwrap();
        audio.render(&mut left, &mut right);
        assert_eq!(control.status().sample, held + i64::from(block));
        assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
    }
}

#[test]
fn a_fader_edit_while_paused_is_settled_before_playback_resumes() {
    let (project, track) = Project::new().add_track("mute before play");
    let mute = project.tracks()[0].devices[0]
        .parameters
        .iter()
        .find(|parameter| parameter.key == echode_model::ParamKey::Mute)
        .unwrap()
        .id;
    let muted = project.set_parameter(mute, 1.0).unwrap();
    for block in BLOCKS {
        let (mut control, mut audio) = clip_player(
            &project,
            block as usize,
            vec![stereo_clip(track, vec![0.25; 2048], vec![-0.25; 2048])],
        );
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        audio.render(&mut left, &mut right);
        assert!(left.iter().all(|sample| *sample == 0.25));
        control
            .command(echode_engine::PlaybackCommand::Pause)
            .unwrap();
        audio.render(&mut left, &mut right);
        control.replace(&muted).unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        audio.render(&mut left, &mut right);
        assert!(!control.status().failed);
        assert!(
            left.iter().chain(&right).all(|sample| *sample == 0.0),
            "muted audio leaked at resume for block {block}"
        );
    }
}

#[test]
fn an_initial_position_matches_a_queued_locate_for_every_block_size() {
    let project = project();
    let sample = 24_000;
    let frames = 2_048;
    let render = |block: usize, initial: bool| {
        let (mut control, mut audio) = if initial {
            echode_engine::playback_pair_at(&project, RATE, block, sample).unwrap()
        } else {
            let (mut control, audio) = echode_engine::playback_pair(&project, RATE, block).unwrap();
            control
                .command(echode_engine::PlaybackCommand::Locate(sample))
                .unwrap();
            (control, audio)
        };
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
        let mut output = Vec::new();
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block);
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count;
        }
        assert_eq!(control.status().sample, sample + frames as i64);
        assert!(!control.status().failed);
        output
    };
    let reference = render(512, false);
    assert!(reference.iter().any(|sample| *sample != 0.0));
    for block in BLOCKS {
        assert_eq!(
            render(block as usize, true),
            reference,
            "initial block {block}"
        );
    }
}

#[test]
fn product_playback_matches_offline_export_for_every_block_size() {
    let project = project();
    let frames = 100000u32;
    let mut expected = vec![0.0; frames as usize * 2];
    echode_engine::render_offline(&project, RATE, frames, &mut expected).unwrap();
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(expected.len());
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert_eq!(output, expected, "product block {block}");
    }
}

#[test]
fn product_loop_and_metronome_match_across_block_sizes() {
    let project = project();
    let end =
        echode_engine::tick_to_sample(Tick::new(TICKS_PER_QUARTER), project.tempo(), RATE).unwrap();
    let frames = 48_000u32;
    let mut reference: Option<Vec<f32>> = None;
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        control
            .command(echode_engine::PlaybackCommand::SetLoop { start: 0, end })
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::SetMetronome(true))
            .unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(frames as usize * 2);
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert!(control.status().sample <= end);
        assert!(output.iter().any(|sample| *sample != 0.0));
        if let Some(reference) = &reference {
            assert_eq!(&output, reference, "loop block {block}");
        } else {
            reference = Some(output);
        }
    }
}

#[test]
fn product_audio_clips_match_across_block_sizes() {
    let (project, track) = Project::new().add_track("audio");
    let audio = AudioRef::try_new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        RATE,
        1,
        64,
    )
    .unwrap();
    let project = project
        .place_audio(track, Id::generate(), Id::generate(), Tick::new(0), audio)
        .unwrap();
    let samples: Arc<[f32]> = (0..64).map(|index| index as f32 / 64.0).collect();
    let track_raw = project.tracks()[0].id.raw();
    let frames = 200u32;
    let mut reference: Option<Vec<f32>> = None;
    for block in BLOCKS {
        let (mut control, mut audio) =
            echode_engine::playback_pair(&project, RATE, block as usize).unwrap();
        let samples = Arc::clone(&samples);
        control.set_clip_loader(Arc::new(move |_project, _rate| {
            Ok(vec![echode_engine::LoadedClip {
                track: track_raw,
                start_sample: 0,
                samples: Arc::clone(&samples),
                right: None,
                fade_in: 8,
                fade_out: 8,
            }])
        }));
        control.replace(&project).unwrap();
        control
            .command(echode_engine::PlaybackCommand::Play)
            .unwrap();
        let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
        let mut output = Vec::with_capacity(frames as usize * 2);
        let mut remaining = frames;
        while remaining > 0 {
            let count = remaining.min(block) as usize;
            audio.render(&mut left[..count], &mut right[..count]);
            for (left, right) in left.iter().zip(&right).take(count) {
                output.extend([*left, *right]);
            }
            remaining -= count as u32;
        }
        assert!(!control.status().failed);
        assert!(output.iter().any(|sample| *sample != 0.0));
        if let Some(reference) = &reference {
            assert_eq!(&output, reference, "audio clip block {block}");
        } else {
            reference = Some(output);
        }
    }
}

fn tempo() -> TempoMap {
    TempoMap::new(Tempo::quarters_per_minute(120).unwrap())
}

fn quarters(value: f64) -> Tick {
    Tick::new((value * TICKS_PER_QUARTER as f64) as i64)
}

fn note(start: f64, length: f64, pitch: u8) -> NoteDraft {
    NoteDraft {
        pitch,
        velocity: 100,
        start: quarters(start),
        length: quarters(length),
    }
}

/// Two tracks with overlapping notes; one note runs across the one-quarter loop end.
fn project() -> Project {
    let (project, lead) = Project::new().add_track("lead");
    let (project, bass) = project.add_track("bass");
    let mut project = project;
    for draft in [note(0.0, 1.0, 60), note(0.5, 2.0, 64), note(3.0, 1.0, 67)] {
        project = project.insert_note(lead, draft).unwrap().0;
    }
    for draft in [note(0.25, 0.25, 48), note(0.75, 1.5, 52)] {
        project = project.insert_note(bass, draft).unwrap().0;
    }
    project
}

fn empty_span() -> PlayedSpan {
    PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }
}

fn silent_event() -> Event {
    Event::note_off(0, 0, 0, 0)
}

/// Pulls `frames` in blocks of `block` and hands each block's spans and length to `render`.
fn drive(
    frames: u32,
    block: u32,
    looped: Option<(Tick, Tick)>,
    mut render: impl FnMut(&[PlayedSpan], usize, &mut Vec<f32>),
) -> Vec<f32> {
    let mut transport = Transport::new(RATE, tempo()).unwrap();
    transport.set_loop(looped).unwrap();
    transport.play();
    let mut spans = [empty_span(); 4];
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        let length = (frames - done).min(block);
        let count = transport.pull(length, &mut spans).unwrap();
        render(&spans[..count], length as usize, &mut out);
        done += length;
    }
    out
}

/// The test synth on every track, mixed to stereo at unity.
fn render_synth(
    project: &Project,
    frames: u32,
    block: u32,
    looped: Option<(Tick, Tick)>,
) -> Vec<f32> {
    let tracks = project.tracks();
    let mut voices: Vec<Vec<SynthVoice>> = tracks
        .iter()
        .map(|_| {
            (0..VOICES_PER_TRACK)
                .map(|_| SynthVoice::new(RATE).unwrap())
                .collect()
        })
        .collect();
    let mut events = vec![silent_event(); 256];
    let mut track_events = vec![silent_event(); 256];
    let mut planes = vec![vec![0.0f32; block as usize]; tracks.len()];
    let faders = vec![TrackFader::unity(); tracks.len()];
    let mut peaks = vec![0.0f32; tracks.len()];
    let mut left = vec![0.0f32; block as usize];
    let mut right = vec![0.0f32; block as usize];
    drive(frames, block, looped, |spans, length, out| {
        let count = schedule(project, project.tempo(), RATE, spans, &mut events).unwrap();
        for (index, _track) in tracks.iter().enumerate() {
            let mut kept = 0;
            for event in events[..count]
                .iter()
                .filter(|event| event.track_key() == Some(index as u32))
            {
                track_events[kept] = *event;
                kept += 1;
            }
            let plane = &mut planes[index][..length];
            plane.fill(0.0);
            render_test_synth(&track_events[..kept], &mut voices[index], RATE, plane).unwrap();
        }
        let refs: Vec<&[f32]> = planes.iter().map(|plane| &plane[..length]).collect();
        mix_tracks(
            &refs,
            &faders,
            &mut left[..length],
            &mut right[..length],
            &mut peaks,
        )
        .unwrap();
        for frame in 0..length {
            out.push(left[frame]);
            out.push(right[frame]);
        }
    })
}

fn assert_same(reference: &[f32], other: &[f32], block: u32) {
    assert_eq!(
        reference.len(),
        other.len(),
        "length differs at block {block}"
    );
    if let Some(index) = reference
        .iter()
        .zip(other)
        .position(|(left, right)| left.to_bits() != right.to_bits())
    {
        panic!(
            "block {block}: sample {index} is {} but block {} gave {}",
            other[index], BLOCKS[0], reference[index]
        );
    }
}

fn assert_block_independent(render: impl Fn(u32) -> Vec<f32>) {
    let reference = render(BLOCKS[0]);
    assert!(
        reference.iter().any(|sample| *sample != 0.0),
        "the render is silent"
    );
    for block in &BLOCKS[1..] {
        assert_same(&reference, &render(*block), *block);
    }
}

#[test]
fn a_clipped_note_does_not_depend_on_the_block_size() {
    let (project, track) = Project::new().add_track("piano");
    let (project, _) = project.insert_note(track, note(0.0, 4.0, 60)).unwrap();
    let clip = project.tracks()[0].clips[0].id;
    let project = project
        .resize_clip(track, clip, Tick::new(0), quarters(1.0))
        .unwrap();
    assert_block_independent(|block| render_synth(&project, 48_000, block, None));
}

#[test]
fn the_test_synth_does_not_depend_on_the_block_size() {
    let project = project();
    assert_block_independent(|block| render_synth(&project, 36_000, block, None));
}

#[test]
fn a_loop_does_not_depend_on_the_block_size() {
    let project = project();
    let looped = Some((Tick::new(0), quarters(1.0)));
    assert_block_independent(|block| render_synth(&project, 60_000, block, looped));
}

#[test]
fn audio_clip_edges_do_not_depend_on_the_block_size() {
    let samples = vec![0.5f32; 30_000];
    assert_block_independent(|block| {
        drive(36_000, block, None, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            let clip = AudioClip {
                start_sample: 1_000,
                samples: &samples,
            };
            render_audio_clips(&[clip], spans, RATE, &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}

#[test]
fn audit_eng2_the_metronome_does_not_depend_on_the_block_size() {
    assert_block_independent(|block| {
        drive(48_000, block, None, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            render_metronome(spans, &tempo(), Meter::four_four(), RATE, &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}

#[test]
fn audit_eng2_the_loop_seam_does_not_depend_on_the_block_size() {
    let samples = vec![0.5f32; 30_000];
    let looped = Some((Tick::new(0), quarters(1.0)));
    assert_block_independent(|block| {
        drive(60_000, block, looped, |spans, length, out| {
            let mut buffer = vec![0.0f32; length];
            let clip = AudioClip {
                start_sample: 0,
                samples: &samples,
            };
            render_audio_clips(&[clip], spans, RATE, &mut buffer).unwrap();
            fade_loop_seam(spans, seam_fade_frames(RATE), &mut buffer).unwrap();
            out.extend_from_slice(&buffer);
        })
    });
}

#[test]
fn live_transport_declick_is_independent_of_blocks_and_leaves_steady_audio_unchanged() {
    use echode_engine::OutputDeclick;
    for rate in [48_000, 96_000] {
        let fade = (rate / 200) as usize;
        let mut reference = None;
        for block in BLOCKS {
            let mut declick = OutputDeclick::new(rate);
            let mut output = Vec::new();
            // DC makes transport steps independent of oscillator phase. Stereo
            // asymmetry catches a shared or swapped correction. Each change can
            // interrupt an unfinished ramp; no lookahead buffers are involved.
            for (count, value, changed) in [
                (fade + 31, [0.25, -0.5], true),
                (fade / 2, [0.0, 0.0], true),
                (fade + 19, [-0.125, 0.375], true),
                (fade + 23, [0.0, 0.0], true),
            ] {
                for offset in (0..count).step_by(block as usize) {
                    let frames = (count - offset).min(block as usize);
                    let mut left = vec![value[0]; frames];
                    let mut right = vec![value[1]; frames];
                    declick.process(&mut left, &mut right, changed && offset == 0);
                    for (index, (l, r)) in left.into_iter().zip(right).enumerate() {
                        if offset + index >= fade - 1 {
                            assert_eq!([l, r], value);
                        }
                        output.push([l, r]);
                    }
                }
            }
            assert_eq!(output[0], [0.0, 0.0]);
            assert_eq!(output[fade + 31], [0.25, -0.5]);
            assert_eq!(output.last().copied(), Some([0.0, 0.0]));
            if let Some(expected) = &reference {
                assert_eq!(&output, expected, "block {block}");
            } else {
                reference = Some(output);
            }
        }
    }
}

#[test]
fn live_stop_fades_the_continuing_waveform_independently_of_blocks() {
    use echode_engine::{OutputDeclick, PlaybackCommand, playback_pair};
    for rate in [48_000, 96_000] {
        let fade = (rate / 200) as usize;
        let stop = rate as usize / 50;
        let pcm: Arc<[f32]> = (0..rate as usize)
            .map(|i| 0.25 * (i as f32 * std::f32::consts::TAU * 437.0 / rate as f32).sin())
            .collect();
        let mut reference = None;
        for block in BLOCKS {
            let (project, track) = Project::new().add_track("waveform");
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            let clip = Arc::clone(&pcm);
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![echode_engine::LoadedClip {
                    track: track.raw(),
                    start_sample: 0,
                    samples: Arc::clone(&clip),
                    right: Some(Arc::clone(&clip)),
                    fade_in: 0,
                    fade_out: 0,
                }])
            }));
            control.replace(&project).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut declick = OutputDeclick::new(rate);
            let mut position = 0;
            while position < stop {
                let frames = (stop - position).min(block as usize);
                let mut left = vec![0.0; frames];
                let mut right = left.clone();
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                declick.process(&mut left, &mut right, position == 0);
                position += frames;
            }
            control.command(PlaybackCommand::Stop).unwrap();
            let mut output = Vec::new();
            for offset in (0..fade + block as usize).step_by(block as usize) {
                let frames = (fade + block as usize - offset).min(block as usize);
                let mut left = vec![0.0; frames];
                let mut right = left.clone();
                audio.render_live_gated_until(
                    &mut left,
                    &mut right,
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                if let Some(age) = audio.stop_fade_offset() {
                    declick.process_stop(&mut left, &mut right, age);
                } else {
                    declick.process(&mut left, &mut right, false);
                }
                output.extend(left);
                assert_eq!(control.status().sample as usize, stop);
            }
            output.truncate(fade);
            assert!((output[0] - pcm[stop]).abs() < 1.0e-7);
            assert_eq!(output.last(), Some(&0.0));
            if let Some(expected) = &reference {
                assert_eq!(&output, expected, "block {block}");
            } else {
                reference = Some(output);
            }
        }
    }
}

#[test]
fn live_stop_does_not_start_future_clips_or_metronome_beats() {
    use echode_engine::{PlaybackCommand, playback_pair_at};
    let frames = 64; // Fixture-only buffer size.
    let (project, track) = Project::new().add_track("future audio");
    let (mut control, mut audio) =
        playback_pair_at(&project, RATE, frames, RATE as i64 / 2 - frames as i64).unwrap();
    control.set_clip_loader(Arc::new(move |_, _| {
        Ok(vec![echode_engine::LoadedClip {
            track: track.raw(),
            start_sample: RATE as i64 / 2,
            samples: vec![1.0; 512].into(),
            right: None,
            fade_in: 0,
            fade_out: 0,
        }])
    }));
    control.replace(&project).unwrap();
    control
        .command(PlaybackCommand::SetMetronome(true))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    audio.render_live_gated_until(
        &mut left,
        &mut right,
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        true,
    );
    assert!(left.iter().all(|value| *value == 0.0));
    control.command(PlaybackCommand::Stop).unwrap();
    for _ in 0..6 {
        audio.render_live_gated_until(
            &mut left,
            &mut right,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            true,
        );
        assert!(left.iter().chain(&right).all(|value| *value == 0.0));
        assert_eq!(control.status().sample, RATE as i64 / 2);
    }
}

#[test]
fn live_stop_preserves_the_active_loop_seam_instead_of_restoring_full_volume() {
    use echode_engine::{OutputDeclick, PlaybackCommand, playback_pair};
    for rate in [48_000, 96_000] {
        let fade = (rate / 200) as usize;
        let end = fade * 8;
        for stop in [fade / 2, end - fade / 2] {
            let mut reference = None;
            for block in BLOCKS {
                let (project, track) = Project::new().add_track("loop seam");
                let (mut control, mut audio) =
                    playback_pair(&project, rate, block as usize).unwrap();
                // This fixture's clip length covers the loop and the stop tail.
                control.set_clip_loader(Arc::new(move |_, _| {
                    Ok(vec![echode_engine::LoadedClip {
                        track: track.raw(),
                        start_sample: 0,
                        samples: vec![0.25; end + fade].into(),
                        right: Some(vec![-0.5; end + fade].into()),
                        fade_in: 0,
                        fade_out: 0,
                    }])
                }));
                control
                    .command(PlaybackCommand::SetLoop {
                        start: 0,
                        end: end as i64,
                    })
                    .unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let mut position = 0;
                while position < stop {
                    let frames = (stop - position).min(block as usize);
                    let mut left = vec![0.0; frames];
                    let mut right = left.clone();
                    audio.render_live_gated_until(
                        &mut left,
                        &mut right,
                        std::time::Instant::now() + std::time::Duration::from_secs(1),
                        true,
                    );
                    position += frames;
                }
                control.command(PlaybackCommand::Stop).unwrap();
                let mut declick = OutputDeclick::new(rate);
                let mut output = Vec::new();
                for offset in (0..fade + block as usize).step_by(block as usize) {
                    let frames = (fade + block as usize - offset).min(block as usize);
                    let mut left = vec![0.0; frames];
                    let mut right = left.clone();
                    audio.render_live_gated_until(
                        &mut left,
                        &mut right,
                        std::time::Instant::now() + std::time::Duration::from_secs(1),
                        true,
                    );
                    if let Some(age) = audio.stop_fade_offset() {
                        declick.process_stop(&mut left, &mut right, age);
                    }
                    for (local, (left, right)) in left.iter().zip(&right).enumerate() {
                        let age = offset + local;
                        let sample = stop + age;
                        let seam = if sample >= end {
                            0.0
                        } else {
                            (sample as f64 / (fade - 1) as f64).min(1.0)
                                * ((end - 1 - sample) as f64 / (fade - 1) as f64).min(1.0)
                        };
                        let gain = if age >= fade - 1 {
                            0.0
                        } else {
                            0.5 * (1.0
                                + (std::f64::consts::PI * age as f64 / (fade - 1) as f64).cos())
                        };
                        for (value, amplitude) in [(*left, 0.25), (*right, -0.5)] {
                            let expected = amplitude * seam * gain;
                            assert!(
                                (f64::from(value) - expected).abs() < 1.0e-7,
                                "stop restored loop volume at {rate}/{block}, stop {stop}, age {age}: {value} != {expected}"
                            );
                        }
                    }
                    output.extend(left);
                    assert!(!control.status().failed);
                    assert_eq!(control.status().sample as usize, stop);
                }
                output.truncate(fade);
                if let Some(expected) = &reference {
                    assert_eq!(&output, expected);
                } else {
                    reference = Some(output);
                }
            }
        }
    }
}

#[test]
fn cached_stop_does_not_start_a_later_region_or_pass_the_loop_end() {
    use echode_engine::{CachedTrack, PlaybackCommand, playback_pair};
    for rate in [48_000u32, 96_000] {
        for block in BLOCKS {
            let (project, track) = Project::new().add_track("late cache");
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            let late = CachedTrack {
                track: track.raw(),
                sample_rate: rate,
                start_sample: 1_000,
                samples: vec![0.5; 2_000].into(),
                right: Some(vec![0.5; 2_000].into()),
            };
            control
                .set_cached_tracks(std::slice::from_ref(&late))
                .unwrap();
            control.replace(&project).unwrap();
            let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            for _ in 0..4 {
                audio.render(&mut left, &mut right);
                control.service();
            }
            control.command(PlaybackCommand::Play).unwrap();
            let stop = 80usize;
            let mut heard = 0usize;
            while heard < stop {
                let n = (stop - heard).min(block as usize);
                audio.render_live_gated_until(
                    &mut left[..n],
                    &mut right[..n],
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                heard += n;
            }
            control.command(PlaybackCommand::Stop).unwrap();
            let fade = (rate / 200) as usize;
            for offset in (0..fade).step_by(block as usize) {
                let n = (fade - offset).min(block as usize);
                audio.render_live_gated_until(
                    &mut left[..n],
                    &mut right[..n],
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                assert!(
                    left[..n]
                        .iter()
                        .chain(&right[..n])
                        .all(|sample| *sample == 0.0),
                    "unstarted cache sounded during the stop tail"
                );
            }

            let (project, track) = Project::new().add_track("loop cache");
            let (mut control, mut audio) = playback_pair(&project, rate, block as usize).unwrap();
            let mut inside = vec![0.4f32; 800];
            inside[200..].fill(0.9);
            let cache = CachedTrack {
                track: track.raw(),
                sample_rate: rate,
                start_sample: 0,
                samples: inside.clone().into(),
                right: Some(inside.into()),
            };
            control
                .set_cached_tracks(std::slice::from_ref(&cache))
                .unwrap();
            control.replace(&project).unwrap();
            control
                .command(PlaybackCommand::SetLoop { start: 0, end: 200 })
                .unwrap();
            let (mut left, mut right) = (vec![0.0; block as usize], vec![0.0; block as usize]);
            for _ in 0..4 {
                audio.render(&mut left, &mut right);
                control.service();
            }
            control.command(PlaybackCommand::Play).unwrap();
            let stop = 190usize;
            let mut heard = 0usize;
            while heard < stop {
                let n = (stop - heard).min(block as usize);
                audio.render_live_gated_until(
                    &mut left[..n],
                    &mut right[..n],
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                heard += n;
            }
            control.command(PlaybackCommand::Stop).unwrap();
            let mut tail = Vec::new();
            for offset in (0..fade).step_by(block as usize) {
                let n = (fade - offset).min(block as usize);
                audio.render_live_gated_until(
                    &mut left[..n],
                    &mut right[..n],
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                    true,
                );
                tail.extend_from_slice(&left[..n]);
            }
            assert!(
                tail.iter()
                    .take(10)
                    .all(|sample| (*sample - 0.4).abs() < 1.0e-5),
                "tail start {tail:?}"
            );
            assert!(tail.iter().skip(10).all(|sample| *sample == 0.0));
            assert!(tail.iter().all(|sample| (*sample - 0.9).abs() > 0.2));
        }
    }
}
