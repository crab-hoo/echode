use super::*;

fn observation(millis: u64, callbacks: u32) -> Observation {
    Observation {
        elapsed: Duration::from_millis(millis),
        wall_elapsed: Some(Duration::from_millis(millis)),
        callbacks,
        output: Ok(123),
        interrupted: false,
    }
}

fn outcome(observations: Vec<Observation>, duration: Duration) -> MeasurementEnd {
    let mut observations = observations.into_iter();
    measure_progress(
        duration,
        123,
        || observations.next().expect("measurement did not terminate"),
        || {},
        || {},
    )
}

fn report(outcome: MeasurementEnd, callbacks: u32) -> Report {
    Report {
        requested_seconds: 600,
        measured_for: Duration::from_secs(600),
        sample_rate: TARGET_RATE,
        buffer_frames: BLOCK_FRAMES as u32,
        callbacks,
        late: 0,
        gaps: 0,
        warmup_late: 0,
        max_ns: 300_000,
        p50_ns: 200_000,
        p99_ns: 250_000,
        budget_ns: 1_333_333,
        workgroup_failures: 0,
        policy_failures: 0,
        workgroup_parallel: 4,
        workers: 3,
        outcome,
    }
}

#[test]
fn measurement_geometry_uses_the_device_rate_and_reserves_the_whole_run() {
    let (budget, slots) = measurement_geometry(96_000.0, 10, false).unwrap();
    assert_eq!(budget, 666_666);
    assert!(slots >= 15_000 + WARMUP_CALLBACKS as usize);
    let (budget, _) = measurement_geometry(44_100.0, 10, false).unwrap();
    assert_eq!(budget, 1_451_247);
    assert!(measurement_geometry(96_000.0, 10, true).is_err());
    assert!(measurement_geometry(48_000.0, 600, true).is_ok());
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(measurement_geometry(rate, 10, false).is_err());
    }
    assert!(measurement_geometry(96_000.0, u64::MAX, false).is_err());
}

#[test]
fn contract_engine_measurement_requires_progress_and_completion() {
    // Synthetic callback counts and elapsed times exercise the real control
    // loop without hardware or wall-clock timing. They are not engine caps.
    let complete = outcome(
        (0..=5)
            .map(|step| observation(step * 200, step as u32 * 150))
            .collect(),
        Duration::from_secs(1),
    );
    assert_eq!(complete, MeasurementEnd::Completed);
    assert!(report(complete, 750).passed());

    let stalled = outcome(
        vec![
            observation(0, 0),
            observation(100, 77_386),
            observation(2000, 77_386),
            observation(2100, 77_386),
        ],
        Duration::from_secs(600),
    );
    assert_eq!(stalled, MeasurementEnd::CallbacksStopped);
    assert!(
        !report(stalled, 77_386).passed(),
        "nonempty timing samples hid a stopped callback"
    );

    let cold = outcome(
        vec![observation(0, 0), observation(2000, 0)],
        Duration::from_secs(600),
    );
    assert_eq!(cold, MeasurementEnd::CallbacksStopped);
    assert!(!report(cold, 0).passed());

    let interrupted = outcome(
        vec![
            observation(0, 0),
            Observation {
                interrupted: true,
                ..observation(400, 300)
            },
        ],
        Duration::from_secs(600),
    );
    assert_eq!(interrupted, MeasurementEnd::Interrupted);
    assert!(
        !report(interrupted, 300).passed(),
        "an interrupted run was treated as complete"
    );

    let changed = outcome(
        vec![
            observation(0, 0),
            Observation {
                output: Ok(456),
                ..observation(400, 300)
            },
        ],
        Duration::from_secs(600),
    );
    assert_eq!(
        changed,
        MeasurementEnd::DefaultOutputChanged {
            expected: 123,
            actual: 456
        }
    );
    assert!(!report(changed, 300).passed());
    let unavailable = outcome(
        vec![Observation {
            output: Err(-50),
            ..observation(400, 300)
        }],
        Duration::from_secs(600),
    );
    assert_eq!(unavailable, MeasurementEnd::DeviceUnavailable(-50));
    assert!(!report(unavailable, 300).passed());
    let resumed = outcome(
        vec![
            observation(0, 0),
            Observation {
                wall_elapsed: Some(Duration::from_secs(30)),
                ..observation(400, 300)
            },
        ],
        Duration::from_secs(600),
    );
    assert_eq!(resumed, MeasurementEnd::ClockDiscontinuity);
    assert!(!report(resumed, 300).passed());

    assert!(!report(MeasurementEnd::Completed, WARMUP_CALLBACKS).passed());
    let mut partial = report(MeasurementEnd::Completed, 77_386);
    partial.measured_for = Duration::from_secs(103);
    assert!(
        !partial.passed(),
        "partial elapsed time was treated as a complete measurement"
    );
    let mut invalid = report(MeasurementEnd::Completed, 750);
    invalid.late = 1;
    assert!(!invalid.passed());
    invalid.late = 0;
    invalid.gaps = 1;
    assert!(!invalid.passed());
}
