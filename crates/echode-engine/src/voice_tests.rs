use super::*;

fn span(start: i64, frames: u32, offset: u32, release_at: Option<i64>) -> PlayedSpan {
    PlayedSpan {
        start_sample: start,
        frames,
        block_offset: offset,
        release_at,
        loop_end: None,
        wrap_from: None,
    }
}

#[test]
fn a_track_can_switch_from_live_devices_to_a_cache() {
    let live = [0.25, 0.5, 0.75, 1.0];
    let spans = [span(10, 4, 0, None)];
    let mut from_devices = [0.0; 4];
    render_track(TrackSource::Live, &live, &spans, &mut from_devices).unwrap();
    assert_eq!(from_devices, live);

    let cached = [9.0, 8.0, 7.0, 6.0, 5.0];
    let mut from_cache = [0.0; 4];
    render_track(
        TrackSource::Cache {
            samples: &cached,
            origin: 8,
        },
        &live,
        &spans,
        &mut from_cache,
    )
    .unwrap();
    assert_eq!(from_cache, [7.0, 6.0, 5.0, 0.0]);
}

#[test]
fn a_wrapped_block_reads_the_loop_head_from_the_cache() {
    let cached = [1.0, 2.0, 3.0, 4.0];
    let spans = [span(2, 2, 0, None), span(0, 2, 2, Some(4))];
    let mut output = [0.0; 4];
    render_track(
        TrackSource::Cache {
            samples: &cached,
            origin: 0,
        },
        &[],
        &spans,
        &mut output,
    )
    .unwrap();
    assert_eq!(output, [3.0, 4.0, 1.0, 2.0]);
}

#[test]
fn the_loop_head_is_requested_only_as_the_end_approaches() {
    assert_eq!(loop_head_prefetch(0, 0, 1_000, 64, 240), None);
    assert_eq!(
        loop_head_prefetch(950, 0, 1_000, 64, 240),
        Some(LoopHead {
            start_sample: 0,
            frames: 240,
        })
    );
    assert_eq!(loop_head_prefetch(1_000, 0, 1_000, 64, 240), None);
}

#[test]
fn clip_edges_and_the_loop_jump_fade_over_five_milliseconds() {
    assert_eq!(seam_fade_frames(1_000), 5);
    let ones = [1.0; 20];
    let clip = AudioClip {
        start_sample: 0,
        samples: &ones,
    };
    let mut output = [0.0; 20];
    render_audio_clips(&[clip], &[span(0, 20, 0, None)], 1_000, &mut output).unwrap();
    assert_eq!(output[0], 0.0);
    assert_eq!(output[4], 1.0);
    assert_eq!(output[10], 1.0);
    assert_eq!(output[15], 1.0);
    assert_eq!(output[19], 0.0);
    let faded = FadedClip {
        start_sample: 0,
        samples: &ones,
        fade_in: 5,
        fade_out: 5,
    };
    let mut matched = [0.0; 20];
    render_faded_clips(&[faded], &[span(0, 20, 0, None)], &mut matched).unwrap();
    assert_eq!(matched, output);
    let full = FadedClip {
        start_sample: 0,
        samples: &ones,
        fade_in: 0,
        fade_out: 0,
    };
    let mut open = [0.0; 20];
    render_faded_clips(&[full], &[span(0, 20, 0, None)], &mut open).unwrap();
    assert_eq!(open[0], 1.0);
    assert_eq!(open[19], 1.0);

    let mut seam = [1.0; 20];
    let mut outgoing = span(0, 10, 0, None);
    outgoing.loop_end = Some(10);
    let mut incoming = span(0, 10, 10, Some(10));
    incoming.wrap_from = Some(0);
    fade_loop_seam(&[outgoing, incoming], 5, &mut seam).unwrap();
    assert_eq!(seam[4], 1.0);
    assert_eq!(seam[5], 1.0);
    assert_eq!(seam[9], 0.0);
    assert_eq!(seam[10], 0.0);
    assert_eq!(seam[14], 1.0);
    assert_eq!(seam[15], 1.0);
}

#[test]
fn a_block_that_crosses_two_clips_plays_both() {
    let head = [1.0; 4];
    let tail = [1.0; 4];
    let clips = [
        AudioClip {
            start_sample: 0,
            samples: &head,
        },
        AudioClip {
            start_sample: 4,
            samples: &tail,
        },
    ];
    let mut output = [0.0; 8];
    render_audio_clips(&clips, &[span(0, 8, 0, None)], 1, &mut output).unwrap();
    assert_eq!(output[1], 1.0);
    assert_eq!(output[4], 0.0);
    assert_eq!(output[5], 1.0);
}
