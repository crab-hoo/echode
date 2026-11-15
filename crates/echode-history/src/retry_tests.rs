use super::*;

#[test]
fn delays_double_to_five_seconds_and_success_resets_the_episode() {
    let mut retry = BackgroundRetry::default();
    let mut now = Instant::now();
    assert!(retry.ready(now));
    for (index, millis) in [100, 200, 400, 800, 1600, 3200, 5000, 5000]
        .into_iter()
        .enumerate()
    {
        assert_eq!(retry.failed(now, &"unreadable"), index == 0);
        let due = now + Duration::from_millis(millis);
        assert!(!retry.ready(due - Duration::from_nanos(1)));
        assert!(retry.ready(due));
        now = due;
    }
    assert!(retry.failed(now, &"different cause"));
    assert!(!retry.failed(now, &"different cause"));
    retry.succeeded();
    assert!(retry.ready(now));
    assert!(retry.failed(now, &"different cause"));
    assert!(retry.ready(now + FIRST));
}

#[test]
fn a_terminal_failure_is_reported_once_and_never_retried() {
    let mut retry = BackgroundRetry::default();
    let now = Instant::now();
    assert!(retry.stop(&"worker disconnected"));
    assert!(!retry.stop(&"worker disconnected"));
    assert!(retry.stopped());
    assert!(!retry.ready(now + Duration::from_secs(60)));
}
