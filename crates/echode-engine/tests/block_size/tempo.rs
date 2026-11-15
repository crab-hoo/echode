//! Fractional tempo boundaries on both sides of zero must not depend on blocks.
use echode_engine::{PlaybackCommand, PlaybackOptions, playback, sample_to_tick, tick_to_sample};
use echode_model::{Project, TICKS_PER_QUARTER, Tempo, Tick};

#[test]
fn fractional_tempo_origin_and_metronome_match_across_blocks_under_rtsan() {
    let project = Project::new()
        .with_tempo(
            Tick::new(-TICKS_PER_QUARTER),
            Tempo::rational(137, 1).unwrap(),
        )
        .with_tempo(Tick::new(-7), Tempo::rational(175, 3).unwrap())
        .with_tempo(Tick::new(13), Tempo::rational(250, 3).unwrap())
        .with_tempo(
            Tick::new(TICKS_PER_QUARTER / 8),
            Tempo::rational(91, 1).unwrap(),
        );
    // These durations, rates, and buffer lengths bound this regression only.
    for rate in [44_100, 96_000] {
        let start =
            tick_to_sample(Tick::new(-TICKS_PER_QUARTER / 8), project.tempo(), rate).unwrap();
        let frames = rate as usize / 3;
        let mut reference = None;
        for block in [1, 7, 64, 511] {
            let (mut control, mut audio) =
                playback(&project, rate, block, PlaybackOptions::default()).unwrap();
            for command in [
                PlaybackCommand::Locate(start),
                PlaybackCommand::SetMetronome(true),
                PlaybackCommand::Play,
            ] {
                control.command(command).unwrap();
            }
            let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
            let mut output = Vec::with_capacity(frames * 2);
            for offset in (0..frames).step_by(block) {
                let count = block.min(frames - offset);
                audio.render(&mut left[..count], &mut right[..count]);
                for (&left, &right) in left[..count].iter().zip(&right[..count]) {
                    output.extend([left, right]);
                }
            }
            let status = control.status();
            assert!(!status.failed);
            assert_eq!(status.sample, start + frames as i64);
            assert_eq!(
                status.tick,
                sample_to_tick(status.sample, project.tempo(), rate)
                    .unwrap()
                    .get()
            );
            assert!(output.iter().any(|sample| *sample != 0.0));
            if let Some(reference) = &reference {
                assert_eq!(&output, reference, "rate {rate}, block {block}");
            } else {
                reference = Some(output);
            }
        }
    }
}

#[test]
fn dense_transport_boundaries_leave_source_audio_block_independent_under_rtsan() {
    use echode_engine::{LoadedClip, PlaybackCommand, PlaybackOptions, playback};
    use std::sync::Arc;
    let (mut project, track) = Project::new().add_track("source");
    for tick in -20..100 {
        project = project.with_tempo(
            Tick::new(tick * 147),
            Tempo::rational(if tick % 2 == 0 { 120 } else { 60 }, 1).unwrap(),
        );
    }
    // These loop lengths, block shapes and PCM bound this regression only.
    for length in [1, 2, 13, 31] {
        let mut reference = None;
        for block in [1, 7, 64, 127] {
            let (mut control, mut audio) =
                playback(&project, 48_000, block, PlaybackOptions::default()).unwrap();
            control.set_clip_loader(Arc::new(move |_, _| {
                Ok(vec![LoadedClip::new(
                    track.raw(),
                    0,
                    echode_engine::FinitePlanes::new(Arc::from(vec![0.125; 512]), None)
                        .unwrap()
                        .into(),
                )])
            }));
            control
                .command(PlaybackCommand::SetLoop {
                    start: 0,
                    end: length,
                })
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = Vec::new();
            while output.len() < 256 {
                let frames = block.min(256 - output.len());
                let mut left = vec![0.0; frames];
                let mut right = left.clone();
                audio.render(&mut left, &mut right);
                assert!(
                    !control.status().failed,
                    "dense transport rejected a valid block"
                );
                output.extend(left);
            }
            assert!(output.iter().any(|sample| *sample != 0.0));
            if let Some(reference) = &reference {
                assert_eq!(&output, reference, "loop {length}, block {block}");
            } else {
                reference = Some(output);
            }
        }
    }
}
