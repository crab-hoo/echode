use super::*;

// Frozen pre-index model arithmetic, independent of the product helpers. These
// small fixture ranges bound this regression, not supported project sizes.
fn rounded(value: i128, divisor: i128) -> i128 {
    (value + value.signum() * (divisor / 2)) / divisor
}

fn duration(ticks: i128, point: (i64, u32, u32)) -> i128 {
    rounded(ticks * 60_000 * i128::from(point.2), i128::from(point.1))
}

fn inverse(flicks: i128, point: (i64, u32, u32)) -> i128 {
    rounded(flicks * i128::from(point.1), 60_000 * i128::from(point.2))
}

fn covering(points: &[(i64, u32, u32)], tick: i64) -> (i64, u32, u32) {
    points
        .iter()
        .rev()
        .find(|point| point.0 <= tick)
        .copied()
        .unwrap_or(points[0])
}

fn legacy_flick(points: &[(i64, u32, u32)], tick: i64) -> i128 {
    let (mut cursor, end) = (tick.min(0), tick.max(0));
    let mut total = 0;
    while cursor < end {
        let next = points
            .iter()
            .find(|point| point.0 > cursor)
            .map_or(end, |p| p.0.min(end));
        total += duration(
            i128::from(next) - i128::from(cursor),
            covering(points, cursor),
        );
        cursor = next;
    }
    if tick < 0 { -total } else { total }
}

fn legacy_tick(points: &[(i64, u32, u32)], target: i128) -> i128 {
    let (mut tick, mut flick) = (0i64, 0i128);
    loop {
        if target >= 0 {
            let tempo = covering(points, tick);
            let Some(end) = points.iter().find(|point| point.0 > tick).map(|p| p.0) else {
                return i128::from(tick) + inverse(target - flick, tempo);
            };
            let next = flick + duration(i128::from(end) - i128::from(tick), tempo);
            if next >= target {
                return i128::from(tick) + inverse(target - flick, tempo);
            }
            (tick, flick) = (end, next);
        } else {
            let tempo = covering(points, tick - 1);
            let Some(start) = points
                .iter()
                .rev()
                .find(|point| point.0 < tick)
                .map(|p| p.0)
            else {
                return i128::from(tick) - inverse(flick - target, tempo);
            };
            let previous = flick - duration(i128::from(tick) - i128::from(start), tempo);
            if previous <= target {
                return i128::from(start) + inverse(target - previous, tempo);
            }
            (tick, flick) = (start, previous);
        }
    }
}

#[test]
fn indexed_conversions_preserve_the_pre_index_model_rounding() {
    for points in [
        vec![(0, 120, 1)],
        vec![(-39, 137, 1), (-7, 175, 3), (13, 250, 3), (99, 999, 1)],
        vec![(-39, 137, 1), (0, 175, 3), (99, 250, 3)],
        vec![(7, 137, 1), (99, 175, 3)],
        vec![(-39, 137, 1), (-7, 175, 3)],
        vec![(-1, 1, u32::MAX), (1, 999, 1)],
    ] {
        let index = TempoIndex::from_tempos(points.iter().copied()).unwrap();
        for tick in -201..=201 {
            assert_eq!(
                index.tick_to_flick(tick),
                Some(legacy_flick(&points, tick)),
                "{points:?} tick {tick}"
            );
        }
        let mut flicks: Vec<_> = (-301..=301).map(|n| n * 67).collect();
        for &point in &points {
            let boundary = legacy_flick(&points, point.0);
            flicks.extend([boundary - 1, boundary, boundary + 1]);
        }
        for flick in flicks {
            assert_eq!(
                index.flick_to_tick(flick).map(i128::from),
                Some(legacy_tick(&points, flick)),
                "{points:?} flick {flick}"
            );
        }
    }
}

#[test]
fn conversions_search_logarithmically_and_clones_share_the_index() {
    for count in [16, 256, 4096] {
        let index =
            TempoIndex::from_tempos((0..count).map(|i| (i * 997 - count * 500, 137, 1))).unwrap();
        let copy = index.clone();
        assert!(Arc::ptr_eq(&index.segments, &copy.segments));
        for tick in [-1_000_000, -1, 0, 1, 1_000_000] {
            VISITS.with(|visits| visits.set(0));
            let flick = index.tick_to_flick(tick).unwrap();
            let restored = index.flick_to_tick(flick).unwrap();
            assert_eq!(restored, tick);
            let visits = VISITS.with(std::cell::Cell::get);
            let bound = 2 * ((count as usize + 1).ilog2() as usize + 2);
            assert!(
                visits <= bound,
                "{visits} boundary visits for {count} tempos, bound {bound}"
            );
        }
    }
}

#[test]
fn rounding_rejects_invalid_divisors_and_overflow_without_panicking() {
    for (value, divisor, expected) in [(1, 2, 1), (-1, 2, -1), (4, 3, 1), (-4, 3, -1), (0, 1, 0)] {
        assert_eq!(div_round(value, divisor), Some(expected));
    }
    assert_eq!(div_round(1, 0), None);
    assert_eq!(div_round(1, -1), None);
    assert_eq!(div_round(i128::MAX, 2), None);
    assert_eq!(div_round(i128::MIN, 2), None);
    assert_eq!(flicks_for_ticks(1, 0, 1), None);
    assert_eq!(flicks_for_ticks(1, 1, 0), None);
    assert_eq!(ticks_for_flicks(1, 0, 1), None);
    assert_eq!(ticks_for_flicks(1, 1, 0), None);
    assert_eq!(flicks_for_ticks(i128::MAX, 1, 1), None);
    assert_eq!(ticks_for_flicks(i128::MAX, 2, 1), None);
    assert_eq!(flick_to_sample(i128::MAX, u32::MAX), None);
    assert_eq!(flick_to_sample(0, 0), None);
    assert_eq!(sample_to_flick(0, 0), None);
    assert_eq!(
        flick_to_sample(i128::from(FLICKS_PER_SECOND), 48_000),
        Some(48_000)
    );
    assert_eq!(
        sample_to_flick(-48_000, 48_000),
        Some(-i128::from(FLICKS_PER_SECOND))
    );
}

#[test]
fn invalid_maps_are_refused_before_a_binary_search_can_use_them() {
    for points in [
        vec![],
        vec![(0, 0, 1)],
        vec![(0, 1, 0)],
        vec![(0, 1000, 1)],
        vec![(0, 120, 1), (0, 60, 1)],
        vec![(1, 120, 1), (-1, 60, 1)],
    ] {
        assert!(TempoIndex::from_tempos(points).is_none());
    }
    let segment = TempoSegment {
        start_tick: 0,
        start_flick: 0,
        numerator: 120,
        denominator: 1,
    };
    assert!(TempoIndex::new(vec![]).is_none());
    assert!(TempoIndex::new(vec![segment, segment]).is_none());
    assert!(
        TempoIndex::new(vec![
            segment,
            TempoSegment {
                start_tick: 1,
                ..segment
            }
        ])
        .is_none()
    );
    let extreme = TempoIndex::new(vec![TempoSegment {
        start_flick: i128::MAX,
        ..segment
    }])
    .unwrap();
    assert_eq!(extreme.tick_to_sample(1, 48_000), None);
    assert_eq!(extreme.flick_to_tick(i128::MIN), None);
    let slow = TempoIndex::from_tempos([(i64::MIN, 1, u32::MAX), (i64::MAX, 999, 1)]).unwrap();
    assert!(slow.tick_to_flick(i64::MIN).is_some());
    assert!(slow.tick_to_flick(i64::MAX).is_some());
    assert_eq!(slow.tick_to_sample(i64::MAX, u32::MAX), None);
}

#[test]
fn block_periods_floor_before_each_callers_range_policy() {
    assert_eq!(block_period_ticks(64, 48_000, 125, 3), Some(32_000));
    assert_eq!(block_period_ticks(128, 96_000, 125, 3), Some(32_000));
    assert_eq!(block_period_ticks(1, 44_100, 1, 1), Some(22_675));
    for args in [
        (0, 48_000, 1, 1),
        (64, 0, 1, 1),
        (64, 48_000, 0, 1),
        (64, 48_000, 1, 0),
    ] {
        assert_eq!(block_period_ticks(args.0, args.1, args.2, args.3), None);
    }
    assert_eq!(block_period_ticks(1, u128::MAX, 2, 1), None);
    assert_eq!(block_period_ticks(1, 1, 1, 1), Some(1_000_000_000));
}
