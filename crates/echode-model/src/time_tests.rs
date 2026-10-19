use super::*;

fn bpm(value: u32) -> Tempo {
    Tempo::quarters_per_minute(value).unwrap()
}

#[test]
fn one_quarter_at_common_tempos_is_exact() {
    let quarter = Tick::quarters(1).unwrap();
    let at_120 = TempoMap::new(bpm(120)).flicks_at(quarter).unwrap();
    assert_eq!(at_120.get(), FLICKS_PER_SECOND / 2);

    let at_60 = TempoMap::new(bpm(60)).flicks_at(quarter).unwrap();
    assert_eq!(at_60.get(), FLICKS_PER_SECOND);

    let at_100 = TempoMap::new(bpm(100)).flicks_at(quarter).unwrap();
    assert_eq!(at_100.get(), FLICKS_PER_SECOND * 60 / 100);
}

#[test]
fn conversion_roundtrips_across_a_tempo_change() {
    let map = TempoMap::new(bpm(120)).with_tempo(Tick::quarters(4).unwrap(), bpm(60));
    let eight = Tick::quarters(8).unwrap();
    let flicks = map.flicks_at(eight).unwrap();
    // Four quarters at 120 BPM, then four at 60.
    assert_eq!(flicks.get(), FLICKS_PER_SECOND * 2 + FLICKS_PER_SECOND * 4);
    assert_eq!(map.ticks_at(flicks).unwrap(), eight);

    let before = Tick::new(Tick::quarters(4).unwrap().get() - 1);
    let early = map.flicks_at(before).unwrap();
    assert_eq!(early.get(), FLICKS_PER_SECOND * 2 - 500);
}

#[test]
fn placement_lead_uses_the_interval_before_the_window() {
    let map = TempoMap::new(bpm(120)).with_tempo(Tick::quarters(2).unwrap(), bpm(60));
    let start = Tick::quarters(2).unwrap().get();
    let lead = placement_media_lead(start, start, &map, 48_000).unwrap();
    let later = tick_to_sample(Tick::new(start + start), &map, 48_000).unwrap()
        - tick_to_sample(Tick::new(start), &map, 48_000).unwrap();
    // Two quarters at 120 BPM are one second. The same width after the
    // tempo change is two seconds. Trimming the left keeps the first.
    assert_eq!(lead, 48_000);
    assert_eq!(later, 96_000);
    let before_file =
        placement_media_lead(0, -Tick::quarters(1).unwrap().get(), &map, 48_000).unwrap();
    assert_eq!(before_file, -24_000);

    let steady = TempoMap::new(bpm(125));
    let mut differed = false;
    for start in 1..4_000 {
        let origin_lead = placement_media_lead(start, start, &steady, 44_100).unwrap();
        let forward = tick_to_sample(Tick::new(start + start), &steady, 44_100).unwrap()
            - tick_to_sample(Tick::new(start), &steady, 44_100).unwrap();
        if origin_lead != forward {
            differed = true;
            break;
        }
    }
    assert!(
        differed,
        "constant tempo still rounds the two intervals differently"
    );
}

#[test]
fn negative_ticks_use_the_tempo_in_effect() {
    let map = TempoMap::new(bpm(120)).with_tempo(Tick::quarters(-4).unwrap(), bpm(60));
    let tick = Tick::quarters(-4).unwrap();
    let flicks = map.flicks_at(tick).unwrap();
    assert_eq!(flicks.get(), -(FLICKS_PER_SECOND * 4));
    assert_eq!(map.ticks_at(flicks).unwrap(), tick);
    assert_eq!(map.flicks_at(Tick::new(0)).unwrap().get(), 0);
}

#[test]
fn points_are_stored_in_order_and_duplicates_are_refused() {
    let map = TempoMap::try_from_points(vec![
        TempoPoint {
            at: Tick::quarters(4).unwrap(),
            tempo: bpm(60),
        },
        TempoPoint {
            at: Tick::new(0),
            tempo: bpm(120),
        },
    ])
    .unwrap();
    assert_eq!(map.points()[0].at, Tick::new(0));
    assert_eq!(map.points()[1].tempo, bpm(60));
    assert!(TempoMap::try_from_points(vec![]).is_none());
    assert!(
        TempoMap::try_from_points(vec![
            TempoPoint {
                at: Tick::new(0),
                tempo: bpm(120),
            },
            TempoPoint {
                at: Tick::new(0),
                tempo: bpm(90),
            },
        ])
        .is_none()
    );
}

#[test]
fn replacing_a_point_keeps_a_single_entry() {
    let map = TempoMap::new(bpm(120)).with_tempo(Tick::new(0), bpm(90));
    assert_eq!(map.points().len(), 1);
    assert_eq!(map.points()[0].tempo, bpm(90));
}

#[test]
fn meter_bar_lengths_stay_on_tick_boundaries() {
    assert_eq!(
        Meter::four_four().bar_ticks().unwrap().get(),
        TICKS_PER_QUARTER * 4
    );
    assert_eq!(
        Meter::try_new(3, 4).unwrap().bar_ticks().unwrap().get(),
        TICKS_PER_QUARTER * 3
    );
    assert_eq!(
        Meter::try_new(5, 8).unwrap().bar_ticks().unwrap().get(),
        TICKS_PER_QUARTER * 5 / 2
    );
    assert!(Meter::try_new(1, 27).is_none());
    assert!(Meter::try_new(0, 4).is_none());
}

#[test]
fn tempo_rejects_zero_and_extremes() {
    assert!(Tempo::quarters_per_minute(0).is_none());
    assert!(Tempo::quarters_per_minute(1_000).is_none());
    assert_eq!(Tempo::rational(240, 2).unwrap(), bpm(120));
}
