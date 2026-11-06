//! The fixture ranges exercise conversion boundaries, not product limits.
use echode_model::{Flick, Tempo, TempoMap, TempoPoint, Tick};

fn map(points: &[(i64, u32, u32)]) -> TempoMap {
    TempoMap::try_from_points(
        points
            .iter()
            .map(|&(at, numerator, denominator)| TempoPoint {
                at: Tick::new(at),
                tempo: Tempo::rational(numerator, denominator).unwrap(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn audit_fca13_playback_and_model_agree_before_and_after_zero() {
    for points in [
        vec![(-30_000, 137, 1), (0, 120, 1), (20_000, 250, 3)],
        vec![(-30_000, 999, 1), (-7, 175, 3), (13, 91, 1)],
        vec![(7, 137, 1), (20_000, 250, 3)],
        vec![(-30_000, 137, 1), (-7, 175, 3)],
    ] {
        let tempo = map(&points);
        let index = super::tempo_index(&tempo).unwrap();
        let mut ticks: Vec<_> = (-80..=80).map(|tick| tick * 997).collect();
        for &(at, _, _) in &points {
            ticks.extend([at - 1, at, at + 1]);
        }
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            for &tick in &ticks {
                assert_eq!(
                    index.tick_to_sample(tick, rate),
                    crate::tick_to_sample(Tick::new(tick), &tempo, rate),
                    "FCA-13 playback/model tick mismatch: {points:?}, {tick}, {rate}"
                );
            }
            for sample in (-150..=150).map(|sample| sample * 137) {
                assert_eq!(
                    index.sample_to_tick(sample, rate),
                    crate::sample_to_tick(sample, &tempo, rate).map(Tick::get),
                    "FCA-13 playback/model sample mismatch: {points:?}, {sample}, {rate}"
                );
            }
        }
        assert_eq!(index.tick_to_sample(1, 0), None);
        assert_eq!(index.sample_to_tick(1, 0), None);
    }
}

#[test]
fn model_and_playback_keep_segment_rounding_at_fractional_tempos() {
    let tempo = map(&[(0, 137, 1), (1, 175, 3), (2, 250, 3), (17, 999, 1)]);
    // Each whole segment rounds independently, then the final position rounds
    // to samples. Combining the fractions first changes these flick values.
    for (tick, flicks) in [(0, 0), (1, 438), (2, 1467), (17, 12267), (18, 12327)] {
        assert_eq!(tempo.flicks_at(Tick::new(tick)), Some(Flick::new(flicks)));
    }
    let index = super::tempo_index(&tempo).unwrap();
    for tick in -30..100 {
        assert_eq!(
            index.tick_to_sample(tick, 192_000),
            crate::tick_to_sample(Tick::new(tick), &tempo, 192_000)
        );
    }
}
