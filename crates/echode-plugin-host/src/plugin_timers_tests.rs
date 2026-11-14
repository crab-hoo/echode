use super::*;

const MAX_WAIT: Duration = Duration::from_millis(50);

#[test]
fn empty_and_cancelled_schedulers_leave_the_idle_wait_unchanged() {
    let now = Instant::now();
    let mut timers = Timers::default();
    assert_eq!(timers.wait(now, MAX_WAIT), MAX_WAIT);
    assert!(timers.due(now).is_empty());
    let id = timers.register(10, now).unwrap();
    assert!(timers.unregister(id));
    assert!(!timers.unregister(id));
    assert!(!timers.unregister(u32::MAX));
    assert_eq!(timers.wait(now, MAX_WAIT), MAX_WAIT);
    assert!(timers.due(now + MAX_WAIT).is_empty());
}

#[test]
fn deadlines_are_not_early_and_missed_periods_do_not_catch_up() {
    let now = Instant::now();
    let mut timers = Timers::default();
    let id = timers.register(10, now).unwrap();
    assert_eq!(timers.wait(now, MAX_WAIT), Duration::from_millis(10));
    assert!(timers.due(now + Duration::from_millis(9)).is_empty());
    assert_eq!(timers.due(now + Duration::from_millis(10)), [id]);
    let late = now + Duration::from_secs(60);
    assert_eq!(timers.wait(late, MAX_WAIT), Duration::ZERO);
    assert_eq!(timers.due(late), [id]);
    assert!(timers.due(late).is_empty());
    assert_eq!(timers.wait(late, MAX_WAIT), Duration::from_millis(10));
    assert_eq!(timers.due(late + Duration::from_millis(10)), [id]);
}

#[test]
fn a_callback_can_cancel_a_due_timer_without_reusing_its_snapshot_id() {
    let now = Instant::now();
    let mut timers = Timers::default();
    let first = timers.register(1, now).unwrap();
    let second = timers.register(1, now).unwrap();
    let later = now + Duration::from_millis(1);
    let due = timers.due(later);
    assert_eq!(due, [first, second]);
    assert!(timers.unregister(first));
    assert!(timers.unregister(second));
    let replacement = timers.register(1, later).unwrap();
    assert!(!due.contains(&replacement));
    assert!(!timers.entries.contains_key(&second));
    assert!(timers.due(later).is_empty());
    assert_eq!(timers.due(later + Duration::from_millis(1)), [replacement]);
}

#[test]
fn zero_is_rejected_and_the_largest_period_is_not_truncated() {
    let now = Instant::now();
    let mut timers = Timers::default();
    assert_eq!(timers.register(0, now), None);
    assert!(timers.entries.is_empty());
    let id = timers.register(u32::MAX, now).unwrap();
    assert_eq!(id, 0);
    assert_eq!(timers.wait(now, MAX_WAIT), MAX_WAIT);
    let deadline = now + Duration::from_millis(u64::from(u32::MAX));
    assert!(timers.due(deadline - Duration::from_millis(1)).is_empty());
    assert_eq!(timers.due(deadline), [id]);
}

#[test]
fn exhausting_the_clap_id_space_never_wraps_or_returns_the_invalid_id() {
    let now = Instant::now();
    let mut timers = Timers {
        next_id: u32::MAX - 1,
        ..Timers::default()
    };
    let last = timers.register(1, now).unwrap();
    assert_eq!(last, u32::MAX - 1);
    assert!(timers.unregister(last));
    assert_eq!(timers.register(1, now), None);
}

#[test]
fn instances_have_independent_lifetimes_and_registrations() {
    let now = Instant::now();
    let mut first = Timers::default();
    let mut second = Timers::default();
    let first_id = first.register(1, now).unwrap();
    let second_id = second.register(10, now).unwrap();
    assert_eq!(first_id, second_id);
    assert!(first.unregister(first_id));
    drop(first);
    assert_eq!(second.due(now + Duration::from_millis(10)), [second_id]);
}
