use super::*;

/// RTA-08: restarting replaces the mapping, including parent-only abandonment.
#[test]
fn a_restarted_helper_collects_a_generation_abandoned_in_the_old_mapping() {
    let fan = Arc::new(FanIn::create().expect("fan-in"));
    let mut host = spawn_fan(None, None, &fan);
    // This block length only bounds the restart fixture, not product capacity.
    let input = stereo(32, 0.25);
    let abandoned = submit_stereo(&mut host, &input);
    assert_eq!(abandoned, 1);
    host.abandon(abandoned);
    assert!(matches!(
        poll_stereo(&mut host, abandoned, 32).0,
        BlockProgress::Abandoned
    ));

    host.restart().expect("restart with a fresh mapping");
    let generation = submit_stereo(&mut host, &input);
    assert_eq!(
        generation, abandoned,
        "the new mapping starts at generation 1"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = fan.load();
    let output = loop {
        let (progress, output) = poll_stereo(&mut host, generation, 32);
        match progress {
            BlockProgress::Ready { events: 0 } => break output,
            BlockProgress::Pending => {
                seen = fan.wait_until(seen, deadline).expect("restarted block");
            }
            other => panic!("the new mapping inherited old abandonment: {other:?}"),
        }
    };
    assert_eq!(output, input, "the new helper returns the complete block");
    host.shutdown().expect("shutdown");
}
