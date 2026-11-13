use super::*;

fn chain(chain: u64) -> TrackLatency {
    TrackLatency {
        chain,
        ..TrackLatency::default()
    }
}

fn delays(source: u64, instrument: u64, output: u64) -> TrackDelays {
    TrackDelays {
        source,
        instrument,
        output,
    }
}

#[test]
fn top_level_tracks_wait_for_the_latest_one_at_master() {
    let plan = compensate(&[None, None, None], &[chain(100), chain(0), chain(30)]).unwrap();
    assert_eq!(plan.latency, 100);
    assert_eq!(
        plan.tracks,
        [delays(0, 0, 0), delays(0, 0, 100), delays(0, 0, 70)]
    );
}

#[test]
fn children_meet_at_their_parent_before_its_devices() {
    // root{fast, slow} with its own device, then a track outside the folder.
    let parents = [None, Some(0), Some(0), None];
    let latencies = [chain(20), chain(10), chain(50), chain(0)];
    let plan = compensate(&parents, &latencies).unwrap();
    // The root's clips wait for its slowest child; the root arrives 50 + 20.
    assert_eq!(plan.latency, 70);
    assert_eq!(
        plan.tracks,
        [
            delays(50, 50, 0),
            delays(0, 0, 40),
            delays(0, 0, 0),
            delays(0, 0, 70)
        ]
    );
}

#[test]
fn a_folder_instrument_waits_for_slower_children_and_they_wait_for_it() {
    let parents = [None, Some(0)];
    // The instrument is later than the child: the child waits.
    let slow_instrument = TrackLatency {
        instrument: 64,
        chain: 8,
        ..TrackLatency::default()
    };
    let plan = compensate(&parents, &[slow_instrument, chain(16)]).unwrap();
    assert_eq!(plan.tracks, [delays(64, 0, 0), delays(0, 0, 48)]);
    assert_eq!(plan.latency, 72);
    // The child is later than the instrument: the instrument waits.
    let fast_instrument = TrackLatency {
        instrument: 4,
        ..slow_instrument
    };
    let plan = compensate(&parents, &[fast_instrument, chain(16)]).unwrap();
    assert_eq!(plan.tracks, [delays(16, 12, 0), delays(0, 0, 0)]);
    assert_eq!(plan.latency, 24);
}

#[test]
fn a_bypassed_device_keeps_its_place_and_delays_its_own_output() {
    let bypassed = TrackLatency {
        chain: 100,
        bypassed: 100,
        ..TrackLatency::default()
    };
    let plan = compensate(&[None, None], &[bypassed, chain(0)]).unwrap();
    assert_eq!(plan.latency, 100);
    // Its dry audio is early by the device's latency; the other track still
    // waits as if the device ran.
    assert_eq!(plan.tracks, [delays(0, 0, 100), delays(0, 0, 100)]);
}

#[test]
fn tails_add_along_each_path_to_master() {
    let parents = [None, Some(0), Some(1), None];
    let tails = [100, 20, 3, 110].map(|tail| TrackLatency {
        tail,
        ..TrackLatency::default()
    });
    assert_eq!(compensate(&parents, &tails).unwrap().tail, 123);
    let infinite = [u32::MAX, 1, 1, 0].map(|tail| TrackLatency {
        tail,
        ..TrackLatency::default()
    });
    assert_eq!(compensate(&parents, &infinite).unwrap().tail, u32::MAX);
}

#[test]
fn a_deep_chain_is_planned_without_recursion() {
    // The depth only sizes this fixture; folders have no depth limit.
    let depth: usize = 10_000;
    let parents: Vec<_> = (0..depth).map(|position| position.checked_sub(1)).collect();
    let latencies = vec![chain(1); depth];
    let plan = compensate(&parents, &latencies).unwrap();
    assert_eq!(plan.latency, depth as u64);
    assert!(plan.tracks.iter().all(|track| track.output == 0));
    assert_eq!(plan.tracks[0].source, depth as u64 - 1);
}

#[test]
fn an_overflowing_latency_or_a_parent_after_its_child_is_refused() {
    let huge = [chain(u64::MAX), chain(1)];
    assert!(matches!(
        compensate(&[None, Some(0)], &huge),
        Err(PlaybackError::Latency(_))
    ));
    assert!(compensate(&[Some(1), None], &[chain(0), chain(0)]).is_err());
    assert!(compensate(&[None], &[]).is_err());
}
