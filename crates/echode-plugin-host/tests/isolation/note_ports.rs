use super::*;

#[test]
fn note_port_rescans_refresh_lifecycle_events_under_rtsan() {
    let mut host = spawn_plugin(Some("note-ports"), SessionLayout::stereo());
    let wait = |host: &mut PluginHost, stages, activations, ports| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = host
                .control_handle()
                .capture_state()
                .expect("note-port probe");
            assert_eq!(state.len(), 3);
            assert_eq!(
                state[0] & 0x80,
                0,
                "note-port capability probe failed: {state:?}; {}",
                host.stderr_log()
            );
            if state == [stages, activations, ports] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "note-port lifecycle did not converge: {state:?}; {}",
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // Name notifications run while stopped with no window, without reactivation.
    wait(&mut host, 3, 1, 1);
    while let Some(notice) = host.poll_notice().unwrap() {
        assert!(!matches!(notice, PluginNotice::PluginRestarted { .. }));
    }
    // Grow, shrink to no ports, then restore. Each inactive full rescan must
    // replace the reset list, not append to it or retain removed ports.
    for (step, ports) in [1u8, 3, 0, 1].into_iter().enumerate() {
        if step > 0 {
            host.control_handle()
                .restore_state(&[ports])
                .expect("request port change");
            wait(&mut host, 7, step as u8 + 1, ports);
        }
        let notes = [note()];
        let events = if ports == 0 { &[][..] } else { &notes[..] };
        for frames in [1, 32, 127, 256] {
            // These buffers cover this fixture's block sizes, not a product limit.
            let input = stereo(frames, 0.0);
            let mut output = stereo(frames, 0.0);
            for reset in [false, true] {
                if reset {
                    host.request_reset();
                }
                host.process_block(
                    frames,
                    &[&input[0], &input[1]],
                    &mut outs(&mut output),
                    events,
                    &mut [note(); 4],
                    Duration::from_secs(2),
                )
                .expect("notes after rescan");
                assert!(
                    output.iter().flatten().all(|sample| *sample == 0.25),
                    "wrong note/reset stream for {ports} ports: {output:?}"
                );
            }
        }
    }
}
