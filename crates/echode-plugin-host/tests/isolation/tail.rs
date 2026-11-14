use super::*;

#[test]
fn changed_tails_reach_the_parent_including_zero_and_infinite_under_rtsan() {
    let mut host = spawn_plugin(Some("tail-changed"), SessionLayout::stereo());
    assert_eq!(host.tail_samples(), 0);
    for (state, expected) in [(64, 1024), (0, 0), (1, u32::MAX), (64, 1024)] {
        host.control_handle().restore_state(&[state]).unwrap();
        // Only this fixture's block length, not a product channel/frame limit.
        let mut left = [0.0; 32];
        let mut right = [0.0; 32];
        host.process(
            &[&[0.25; 32], &[0.25; 32]],
            &mut [&mut left, &mut right],
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(left, [0.5; 32]);
        assert_eq!(right, [0.5; 32]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let _ = host.poll_notice().unwrap();
            if host.tail_samples() == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "changed tail was not delivered: expected {expected}, got {}; {}",
                host.tail_samples(),
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
