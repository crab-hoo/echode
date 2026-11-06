//! Convert musical time to device samples and back.
//!
//! Rounding happens once for the whole position, using the tempo map's flick
//! value. Exact tempos stay on whole samples.

use echode_model::{Flick, TempoMap, Tick};

pub fn tick_to_sample(tick: Tick, tempo: &TempoMap, sample_rate: u32) -> Option<i64> {
    echode_model::tick_to_sample(tick, tempo, sample_rate)
}

pub fn sample_to_tick(sample: i64, tempo: &TempoMap, sample_rate: u32) -> Option<Tick> {
    let flicks = echode_time::sample_to_flick(sample, sample_rate)?;
    tempo.ticks_at(Flick::new(i64::try_from(flicks).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_model::{Tempo, Tick};

    fn bpm(value: u32) -> TempoMap {
        TempoMap::new(Tempo::quarters_per_minute(value).unwrap())
    }

    #[test]
    fn a_quarter_at_120_bpm_is_half_a_second() {
        let tempo = bpm(120);
        let rate = 48_000;
        assert_eq!(
            tick_to_sample(Tick::quarters(1).unwrap(), &tempo, rate),
            Some(24_000)
        );
        assert_eq!(
            sample_to_tick(24_000, &tempo, rate),
            Some(Tick::quarters(1).unwrap())
        );
        assert_eq!(tick_to_sample(Tick::new(0), &tempo, rate), Some(0));
    }

    #[test]
    fn time_after_a_tempo_change_uses_both_sections() {
        let tempo = TempoMap::new(Tempo::quarters_per_minute(120).unwrap()).with_tempo(
            Tick::quarters(4).unwrap(),
            Tempo::quarters_per_minute(60).unwrap(),
        );
        let rate = 48_000;
        assert_eq!(
            tick_to_sample(Tick::quarters(4).unwrap(), &tempo, rate),
            Some(96_000)
        );
        assert_eq!(
            tick_to_sample(Tick::quarters(5).unwrap(), &tempo, rate),
            Some(144_000)
        );
    }
}
