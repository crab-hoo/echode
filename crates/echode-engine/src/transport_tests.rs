use super::*;
use echode_model::Tempo;

#[test]
fn the_downbeat_transport_is_playing_at_120() {
    let mut transport = Transport::new(
        48_000,
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
    )
    .unwrap();
    transport.play();
    let event = transport.transport_event(24_000, 0, Meter::four_four());
    let Event::Transport(event) = event else {
        panic!("transport");
    };
    assert!(event.playing);
    assert!(!event.loop_active);
    assert_eq!(event.tempo_bpm, 120.0);
    assert_eq!(event.song_pos_beats, 1.0);
    assert_eq!(event.song_pos_seconds, 0.5);
    assert_eq!(event.bar_number, 0);
    assert_eq!(event.signature_numerator, 4);
}

#[test]
fn loops_enter_from_before_but_locates_at_or_after_the_end_bypass_them() {
    let mut transport = transport();
    transport
        .set_loop(Some((
            Tick::quarters(1).unwrap(),
            Tick::quarters(2).unwrap(),
        )))
        .unwrap();
    let empty = PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    };
    // These slots cover this call's two spans only; they are not a transport limit.
    let mut spans = [empty; 2];
    transport.play();
    assert_eq!(transport.pull(48_001, &mut spans).unwrap(), 2);
    assert_eq!(spans[0].start_sample, 0);
    assert_eq!(spans[0].frames, 48_000);
    assert_eq!(spans[1].start_sample, 24_000);
    assert_eq!(transport.position_sample(), 24_001);
    for beat in [2, 3] {
        transport.locate(Tick::quarters(beat).unwrap()).unwrap();
        assert_eq!(transport.pull(512, &mut spans).unwrap(), 1);
        assert_eq!(transport.position_sample(), beat * 24_000 + 512);
        assert_eq!(spans[0].loop_end, None);
        assert_eq!(spans[0].release_at, None);
    }
}

#[test]
fn pausing_at_the_loop_end_preserves_the_crossing_but_locating_there_bypasses_it() {
    let mut transport = transport();
    transport
        .set_loop(Some((
            Tick::quarters(1).unwrap(),
            Tick::quarters(2).unwrap(),
        )))
        .unwrap();
    transport.locate(Tick::quarters(1).unwrap()).unwrap();
    // A single span suffices for each boundary probe below.
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 1];
    transport.play();
    transport.pull(24_000, &mut spans).unwrap();
    assert_eq!(transport.position_sample(), 48_000);
    transport.stop();
    assert_eq!(transport.pull(64, &mut spans).unwrap(), 0);
    transport.play();
    transport.pull(1, &mut spans).unwrap();
    assert_eq!(transport.position_sample(), 24_001);
    assert_eq!(spans[0].release_at, Some(48_000));
    transport.locate(Tick::quarters(2).unwrap()).unwrap();
    transport.pull(1, &mut spans).unwrap();
    assert_eq!(transport.position_sample(), 48_001);
    assert_eq!(spans[0].release_at, None);
}

fn transport() -> Transport {
    Transport::new(
        48_000,
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
    )
    .unwrap()
}

#[test]
fn stop_holds_the_playhead() {
    let mut transport = transport();
    transport.locate(Tick::quarters(1).unwrap()).unwrap();
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 4];
    assert_eq!(transport.pull(64, &mut spans).unwrap(), 0);
    assert_eq!(transport.position_sample(), 24_000);
    transport.play();
    assert_eq!(transport.pull(64, &mut spans).unwrap(), 1);
    assert_eq!(spans[0].start_sample, 24_000);
    assert_eq!(spans[0].frames, 64);
    assert_eq!(transport.position_sample(), 24_064);
}

#[test]
fn a_loop_wraps_once_and_releases_notes_at_the_boundary() {
    let mut transport = transport();
    transport
        .set_loop(Some((Tick::new(0), Tick::quarters(1).unwrap())))
        .unwrap();
    transport.play();
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 4];
    let count = transport.pull(24_064, &mut spans).unwrap();
    assert_eq!(count, 2);
    assert_eq!(spans[0].start_sample, 0);
    assert_eq!(spans[0].frames, 24_000);
    assert_eq!(spans[0].release_at, None);
    assert_eq!(spans[1].start_sample, 0);
    assert_eq!(spans[1].frames, 64);
    assert_eq!(spans[1].block_offset, 24_000);
    assert_eq!(spans[1].release_at, Some(24_000));
    assert_eq!(transport.position_sample(), 64);
}

#[test]
fn a_loop_that_does_not_move_is_rejected() {
    let mut transport = transport();
    let error = transport
        .set_loop(Some((
            Tick::quarters(2).unwrap(),
            Tick::quarters(2).unwrap(),
        )))
        .unwrap_err();
    assert_eq!(error, PlaybackError::InvalidLoop);
    assert!(Transport::new(0, TempoMap::new(Tempo::quarters_per_minute(120).unwrap())).is_err());
}

#[test]
fn an_undersized_span_buffer_does_not_move_the_playhead() {
    let mut transport = transport();
    transport
        .set_loop(Some((Tick::new(0), Tick::quarters(1).unwrap())))
        .unwrap();
    transport.play();
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 1];
    assert_eq!(
        transport.pull(24_064, &mut spans).unwrap_err(),
        PlaybackError::BufferTooSmall
    );
    assert_eq!(transport.position_sample(), 0);
    assert!(!transport.has_wrapped);
}

#[test]
fn transport_can_consume_another_clock_provider() {
    struct Clock(i64);
    impl ClockSource for Clock {
        fn position(&self) -> i64 {
            self.0
        }
        fn locate(&mut self, sample: i64) {
            self.0 = sample;
        }
        fn advance(&mut self, frames: u32) -> Result<(), PlaybackError> {
            self.0 += i64::from(frames);
            Ok(())
        }
    }
    let mut transport = Transport::with_clock(
        48000,
        TempoMap::new(Tempo::quarters_per_minute(120).unwrap()),
        Clock(123),
    )
    .unwrap();
    transport.play();
    let mut spans = [PlayedSpan {
        start_sample: 0,
        frames: 0,
        block_offset: 0,
        release_at: None,
        loop_end: None,
        wrap_from: None,
    }; 1];
    assert_eq!(transport.pull(64, &mut spans).unwrap(), 1);
    assert_eq!(spans[0].start_sample, 123);
    assert_eq!(transport.position_sample(), 187);
}
