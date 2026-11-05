//! Starting playback exactly where a note ends must not strike that note.

use super::*;

fn live(audio: &mut echode_engine::PlaybackAudio, left: &mut [f32], right: &mut [f32]) {
    audio.render_live_gated_until(left, right, Instant::now() + Duration::from_secs(5), true);
}

#[test]
fn contract_a_start_at_a_note_end_does_not_hold_that_note_under_rtsan() {
    let path = plugin_dylib();
    // These block lengths and chains bound the regression, not the engine.
    for frames in [7, 64, 512] {
        for chain in 0..3 {
            for rewind in [false, true] {
                let (project, _, _, mut control, mut audio) =
                    tail_player(&path, frames, chain, "tail");
                // `tail_player` writes one note over the first two quarters.
                let end =
                    tick_to_sample(Tick::quarters(2).unwrap(), project.tempo(), 48_000).unwrap();
                // A stop that returns to the remembered start and a seek both
                // land on the note's off edge before the next play.
                control
                    .command(if rewind {
                        PlaybackCommand::Rewind(end)
                    } else {
                        PlaybackCommand::Locate(end)
                    })
                    .unwrap();
                control.command(PlaybackCommand::Play).unwrap();
                let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
                let drain = 8192usize + control.tail_samples() as usize;
                // Live cleanup holds the clock while its helper receipts are
                // pending. Silent callbacks then consume no score frames, so
                // count actual progress rather than ending the check early.
                let target = end + i64::try_from(drain).unwrap();
                let until = Instant::now() + Duration::from_secs(5);
                while control.status().sample < target {
                    live(&mut audio, &mut left, &mut right);
                    assert!(
                        left.iter().chain(&right).all(|value| *value == 0.0),
                        "a note ending at the start sounded: {frames}/{chain}/{rewind}"
                    );
                    let status = control.status();
                    assert!(
                        !status.failed && (status.playing || audio.cleanup_pending()),
                        "playback failed while draining the note edge: {frames}/{chain}/{rewind}, {status:?}, {:?}",
                        control.plugin_failure()
                    );
                    assert!(
                        Instant::now() < until,
                        "note-edge cleanup/progress did not finish: {frames}/{chain}/{rewind}, {status:?}, {:?}",
                        control.plugin_failure()
                    );
                    if audio.cleanup_pending() {
                        // Only the synthetic callback driver waits, never RT.
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                assert!(
                    control.status().playing,
                    "playback stopped at note end: {frames}/{chain}/{rewind}, {:?}, {:?}",
                    control.status(),
                    control.plugin_failure()
                );
                assert!(control.status().sample >= end);
                assert_eq!(control.status().plugin_missed_blocks, 0);
            }
        }
    }
}
