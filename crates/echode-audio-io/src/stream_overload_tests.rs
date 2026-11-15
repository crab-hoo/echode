use super::*;

#[test]
fn audit_pca10_hal_overloads_reach_the_product_log_without_stopping_output() {
    let capture = echode_log::testing::capture();
    let project = Project::new();
    let tick = 3 * echode_model::TICKS_PER_QUARTER;
    let device = OutputDevice {
        id: 412,
        uid: "overload-output".into(),
        name: "overload-output".into(),
        sample_rate: 48_000,
        buffer_frames: 128,
        safety_frames: 0,
        channels: 2,
        formats: Vec::new(),
    };
    let claim = Claim {
        device,
        listener: None,
        workgroup: None,
    };
    let prepared = Prepared::new(&project, &claim.target(), tick, &TransportSetup::idle()).unwrap();
    let epoch = Arc::new(AtomicU64::new(0));
    let mut stream = OutputStream::assemble(
        claim,
        prepared,
        Start {
            cancellation: TransportCancellation::default(),
            revision: 0,
            package: None,
            hardware_epoch: Arc::clone(&epoch),
            observed_epoch: 0,
            silent: false,
        },
    );
    stream.overloads = Some(crate::overload::Listener::unregistered(412, "output"));
    for _ in 0..3 {
        let _guard = ForbidAlloc::enter();
        stream.overloads.as_ref().unwrap().notify();
    }
    assert!(
        capture.on_this_thread().is_empty(),
        "an IO notification must not log"
    );
    stream.service().unwrap();
    let records = capture.on_this_thread();
    assert_eq!(
        records.len(),
        1,
        "PCA-10: HAL overload notifications never reached the product log"
    );
    let record = &records[0];
    assert_eq!(record["level"], "warn");
    assert!(record["time_ms"].as_u64().is_some());
    let message = record["message"].as_str().unwrap();
    assert!(message.contains("device=412"));
    assert!(message.contains("notifications=3 total=3"));
    assert!(message.contains(&format!("observed_sample=Some({})", stream.status().sample)));
    assert!(message.contains("observed_callbacks=0"));
    assert_eq!(
        epoch.load(Ordering::Acquire),
        0,
        "overload is not a configuration change"
    );
    assert!(!stream.status().failed);
    // This regression pumps one owned 128-frame buffer, not a product limit.
    stream.pump(128);
    assert_eq!(stream.callbacks(), 1);
    stream.overloads.as_ref().unwrap().notify();
    stream.close();
    assert_eq!(
        capture.on_this_thread().len(),
        2,
        "close must flush rate-limited notices"
    );
    assert!(
        capture.on_this_thread().last().unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("observed_callbacks=1")
    );
    stream.close();
    assert_eq!(capture.on_this_thread().len(), 2);
}
