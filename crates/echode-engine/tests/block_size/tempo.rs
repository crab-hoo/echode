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
