use super::*;

#[test]
fn input_time_extrapolates_preroll_then_follows_the_source_trajectory() {
    for origin in [-11, 10, 25, 40] {
        let mut head = Playhead::new(origin, (Some(-5), Some(25)));
        for elapsed in 0..180 {
            for latency in [0, 1, 37, 131] {
                let linear = origin + elapsed - latency as i64;
                let expected = if origin < 25 && linear >= 25 {
                    -5 + (linear - 25) % 30
                } else {
                    linear
                };
                assert_eq!(head.input_sample(latency).unwrap(), expected);
            }
            head.advance(1, 0).unwrap();
        }
    }
    assert!(matches!(
        Playhead::new(i64::MIN, (None, None)).input_sample(1),
        Err(PlaybackError::Time)
    ));
    assert_eq!(
        Playhead::new(i64::MAX, (None, None))
            .input_sample(u64::MAX)
            .unwrap(),
        i64::MIN
    );
}

#[test]
fn playhead_follows_source_history_across_first_entry_and_many_wraps() {
    // These lengths and histories bound only the regression, not playback.
    for length in [1, 2, 13, 64] {
        let (start, end) = (20, 20 + length);
        for origin in [-50, 0, start, end - 1, end, end + 10] {
            let mut history = vec![origin];
            let mut position = origin;
            for _ in 0..700 {
                position += 1;
                if origin < end && position == end {
                    position = start;
                }
                history.push(position);
            }
            for latency in [0, 1, 37, 600, u64::MAX] {
                for block in [1, 7, 64, 512] {
                    let mut head = Playhead::new(origin, (Some(start), Some(end)));
                    assert_eq!(head.sample(), origin);
                    let mut rendered = 0;
                    while rendered < 700 {
                        let count = block.min(700 - rendered);
                        head.advance(count, latency).unwrap();
                        rendered += count;
                        let index = (rendered as u64).saturating_sub(latency) as usize;
                        assert_eq!(
                            head.sample(),
                            history[index],
                            "origin {origin}, loop {start}..{end}, latency {latency}, block {block}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn playhead_arithmetic_does_not_wrap_at_integer_boundaries() {
    let mut head = Playhead::new(i64::MIN, (Some(i64::MIN), Some(i64::MAX)));
    head.advance(usize::MAX, 0).unwrap();
    assert_eq!(head.sample(), i64::MIN);
    assert!(matches!(head.advance(1, 0), Err(PlaybackError::Time)));
    let mut head = Playhead::new(i64::MAX - 10, (None, None));
    head.advance(10, 0).unwrap();
    assert_eq!(head.sample(), i64::MAX);
    head.advance(1, u64::MAX).unwrap();
    assert_eq!(head.sample(), i64::MAX - 10);
}

fn pump(audio: &mut PlaybackAudio, count: usize) {
    let (mut left, mut right) = (vec![0.0; count], vec![0.0; count]);
    audio.render_live_gated_until(
        &mut left,
        &mut right,
        Instant::now() + std::time::Duration::from_secs(10),
        true,
    );
    assert!(!audio.failed());
}

fn displayed(control: &PlaybackControl, project: &Project, sample: i64, rate: u32) {
    assert_eq!(
        control.status().playhead_tick,
        crate::sample_to_tick(sample, project.tempo(), rate)
            .unwrap()
            .get(),
        "displayed sample {sample}: {:?}",
        control.status()
    );
}

#[test]
fn playhead_pause_resume_seek_stop_and_latency_changes_keep_the_render_clock_under_rtsan() {
    // Geometry and synthetic plan latency belong to this status regression only;
    // the real delaying helper and waveform are covered by hosted_playback.
    for rate in [44_100, 96_000] {
        let project = Project::new().with_tempo(
            echode_model::Tick::new(13),
            echode_model::Tempo::rational(91, 1).unwrap(),
        );
        let (mut control, mut audio) =
            playback(&project, rate, 64, PlaybackOptions::default().at(100)).unwrap();
        audio.current.latency = 37;
        control.command(PlaybackCommand::Play).unwrap();
        pump(&mut audio, 17);
        displayed(&control, &project, 100, rate);
        pump(&mut audio, 23);
        assert_eq!(control.status().sample, 140);
        displayed(&control, &project, 103, rate);
        control.command(PlaybackCommand::Pause).unwrap();
        pump(&mut audio, 64);
        // A newly adopted latency must not move the paused display.
        audio.current.latency = 99;
        pump(&mut audio, 64);
        displayed(&control, &project, 103, rate);
        assert_eq!(control.status().sample, 140);
        audio.current.latency = 37;
        control.command(PlaybackCommand::Play).unwrap();
        pump(&mut audio, 17);
        displayed(&control, &project, 120, rate);
        assert_eq!(control.status().sample, 157);
        control.command(PlaybackCommand::Locate(500)).unwrap();
        pump(&mut audio, 7);
        displayed(&control, &project, 500, rate);
        pump(&mut audio, 37);
        displayed(&control, &project, 507, rate);
        audio.current.latency = 50;
        pump(&mut audio, 10);
        displayed(&control, &project, 504, rate);
        audio.current.latency = 0;
        pump(&mut audio, 10);
        displayed(&control, &project, 564, rate);
        control.command(PlaybackCommand::Rewind(20)).unwrap();
        pump(&mut audio, 64);
        displayed(&control, &project, 20, rate);
        assert!(!control.status().playing);
        audio.current.latency = 37;
        control.command(PlaybackCommand::Play).unwrap();
        pump(&mut audio, 64);
        displayed(&control, &project, 47, rate);
    }
}

#[test]
fn playhead_plan_replacement_keeps_progress_and_trajectory_edits_hold_the_new_origin_under_rtsan() {
    let project = Project::new();
    // This geometry bounds only this callback/plan-exchange regression.
    let (mut control, mut audio) =
        playback(&project, 48_000, 64, PlaybackOptions::default()).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    pump(&mut audio, 64);
    let (edited, _) = project.add_track("unrelated");
    control.replace(&edited).unwrap();
    pump(&mut audio, 0); // Adopt without consuming source frames.
    audio.current.latency = 37;
    pump(&mut audio, 64);
    displayed(&control, &edited, 91, 48_000);
    control
        .command(PlaybackCommand::SetLoop {
            start: 200,
            end: 213,
        })
        .unwrap();
    pump(&mut audio, 0);
    audio.current.latency = 37;
    pump(&mut audio, 17);
    displayed(&control, &edited, 128, 48_000);
    pump(&mut audio, 47);
    displayed(&control, &edited, 155, 48_000);
    control.command(PlaybackCommand::ClearLoop).unwrap();
    pump(&mut audio, 0);
    audio.current.latency = 37;
    pump(&mut audio, 7);
    displayed(&control, &edited, 192, 48_000);
    let slower = edited.with_tempo(
        echode_model::Tick::new(0),
        echode_model::Tempo::rational(60, 1).unwrap(),
    );
    let before = control.status().tick;
    control.replace(&slower).unwrap();
    pump(&mut audio, 0);
    audio.current.latency = 37;
    pump(&mut audio, 17);
    assert_eq!(control.status().playhead_tick, before);
}
