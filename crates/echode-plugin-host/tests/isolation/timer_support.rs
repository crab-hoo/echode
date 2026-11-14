use super::*;

#[test]
fn timers_run_on_main_while_idle_and_can_change_their_own_registration_under_rtsan() {
    let mut host = spawn_plugin(Some("timer-support"), SessionLayout::stereo());
    // No audio block or window is needed for timers to run. This is an eventual
    // lifecycle check, not a measurement of the timer's scheduling accuracy.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = host
            .control_handle()
            .capture_state()
            .expect("timer probe state");
        assert_eq!(state.len(), 1);
        assert_eq!(
            state[0] & 0x80,
            0,
            "timer callback failed: {state:?}; {}",
            host.stderr_log()
        );
        if state[0] == 0x0f {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timer probes did not complete: {state:?}; {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for frames in [1, 32, 127, 256] {
        // These buffers cover this fixture's block lengths, not a product limit.
        let input = stereo(frames, 0.1);
        let mut output = [vec![0.0; frames], vec![0.0; frames]];
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .expect("audio after timer callbacks");
        for (source, rendered) in input.iter().zip(&output) {
            for (source, rendered) in source.iter().zip(rendered) {
                assert!((rendered - source * 2.0).abs() < 1.0e-6);
            }
        }
    }
    assert_eq!(host.control_handle().capture_state().unwrap(), [0x0f]);
}
