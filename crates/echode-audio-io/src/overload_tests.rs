use super::*;
use echode_engine::ForbidAlloc;

#[test]
fn bursts_are_counted_coalesced_and_flushed_without_a_wait_under_rtsan() {
    let capture = echode_log::testing::capture();
    let mut listener = Listener::unregistered(413, "output");
    let now = Instant::now();
    assert_eq!(
        Listener::property().mSelector,
        kAudioDeviceProcessorOverload
    );
    assert_eq!(Listener::property().mScope, kAudioObjectPropertyScopeGlobal);
    for _ in 0..100 {
        let _guard = ForbidAlloc::enter();
        listener.notify();
    }
    assert!(capture.on_this_thread().is_empty());
    assert!(listener.notices.last_host_time.load(Ordering::Acquire) > 0);
    listener.report_at(Some(512), 4, false, now);
    listener.notify();
    listener.report_at(Some(640), 5, false, now + Duration::from_millis(999));
    assert_eq!(capture.on_this_thread().len(), 1);
    listener.report_at(Some(768), 6, false, now + Duration::from_secs(1));
    assert_eq!(capture.on_this_thread().len(), 2);
    listener.notify();
    listener.report_at(Some(896), 7, true, now + Duration::from_secs(1));
    let records = capture.on_this_thread();
    assert_eq!(records.len(), 3);
    assert!(
        records[0]["message"]
            .as_str()
            .unwrap()
            .contains("notifications=100 total=100")
    );
    assert!(
        records[2]["message"]
            .as_str()
            .unwrap()
            .contains("notifications=1 total=102")
    );
    listener.report_at(None, 7, true, now);
    assert_eq!(capture.on_this_thread().len(), 3);
}

#[test]
fn a_dispatched_block_owns_its_data_after_removal_and_a_new_device_starts_at_zero_under_rtsan() {
    let old = Listener::unregistered(414, "output");
    let copy = old.block.clone();
    let notices = Arc::clone(&old.notices);
    drop(old);
    let mut property = Listener::property();
    {
        let _guard = ForbidAlloc::enter();
        copy.call((1, NonNull::from(&mut property)));
    }
    assert_eq!(notices.count.load(Ordering::Acquire), 1);
    let next = Listener::unregistered(415, "audition");
    assert_eq!(next.notices.count.load(Ordering::Acquire), 0);
}
