use super::*;

const RATE: u32 = 48_000;
const FRAMES: usize = 128;
/// One host tick is one nanosecond, so expected values read as durations.
const NANOS: Option<(u32, u32)> = Some((1, 1));
const NOW: u64 = 1_000_000_000;

fn ns(value: u64) -> Duration {
    Duration::from_nanos(value)
}

fn block() -> Duration {
    block_duration(FRAMES, RATE)
}

#[test]
fn a_late_entry_ends_at_the_output_time_less_the_safety_offset_and_the_reserve() {
    let cycle = IoCycle::with_timebase(RATE, 32, NANOS);
    let at = Instant::now();
    // HAL woke this block late: only 2 ms remain until its output time,
    // where an on-time entry has one block plus the safety offset.
    let deadline = cycle.deadline(at, NOW, Some(NOW + 2_000_000), FRAMES);
    let safety = block_duration(32, RATE);
    assert_eq!(safety, ns(666_666));
    assert_eq!(block() / 16, ns(166_666));
    assert_eq!(deadline, at + ns(2_000_000) - safety - block() / 16);
    assert!(deadline < at + block());
}

#[test]
fn an_on_time_entry_keeps_one_block_less_the_reserve() {
    let cycle = IoCycle::with_timebase(RATE, 32, NANOS);
    let at = Instant::now();
    let ahead = (block() + block_duration(32, RATE)).as_nanos() as u64;
    let deadline = cycle.deadline(at, NOW, Some(NOW + ahead), FRAMES);
    assert_eq!(deadline, at + block() - block() / 16);
}

#[test]
fn an_output_time_far_ahead_never_extends_past_one_block() {
    let cycle = IoCycle::with_timebase(RATE, 0, NANOS);
    let at = Instant::now();
    let far = NOW + 10 * block().as_nanos() as u64;
    assert_eq!(cycle.deadline(at, NOW, Some(far), FRAMES), at + block());
    // A bogus time stamp saturates instead of overflowing `Instant`.
    let apple = IoCycle::with_timebase(RATE, 0, Some((125, 3)));
    assert_eq!(apple.deadline(at, 0, Some(u64::MAX), FRAMES), at + block());
}

#[test]
fn an_output_time_inside_the_safety_offset_ends_at_entry() {
    let cycle = IoCycle::with_timebase(RATE, 32, NANOS);
    let at = Instant::now();
    assert_eq!(cycle.deadline(at, NOW, Some(NOW + 300_000), FRAMES), at);
    assert_eq!(cycle.deadline(at, NOW, Some(NOW - 1), FRAMES), at);
}

#[test]
fn an_invalid_output_time_or_timebase_keeps_one_block_from_entry() {
    let at = Instant::now();
    let cycle = IoCycle::with_timebase(RATE, 32, NANOS);
    assert_eq!(cycle.deadline(at, NOW, None, FRAMES), at + block());
    let untimed = IoCycle::with_timebase(RATE, 32, None);
    assert_eq!(
        untimed.deadline(at, NOW, Some(NOW + 2_000_000), FRAMES),
        at + block()
    );
}

#[test]
fn host_ticks_use_the_mach_timebase() {
    // Apple silicon counts 24 MHz ticks: 125/3 ns each.
    let cycle = IoCycle::with_timebase(RATE, 0, Some((125, 3)));
    let at = Instant::now();
    // 64,000 ticks are 2,666,666.67 ns, one 128-frame block at 48 kHz.
    let deadline = cycle.deadline(at, NOW, Some(NOW + 64_000), FRAMES);
    assert_eq!(deadline, at + ns(2_666_666) - block() / 16);
    let host = IoCycle::new(RATE, 0);
    assert!(host.timebase.is_some());
    assert!(host_now() > 0);
}
