use super::*;
use echode_realtime::Transport;

fn transport(time: u32, playing: bool) -> Event {
    Event::Transport(Transport {
        time,
        playing,
        recording: playing,
        loop_active: playing,
        tempo_bpm: if playing { 137.5 } else { 93.25 },
        tempo_inc: 0.125,
        song_pos_beats: -3.5,
        song_pos_seconds: -1.25,
        loop_start_beats: -4.0,
        loop_end_beats: 8.0,
        loop_start_seconds: -2.0,
        loop_end_seconds: 4.0,
        bar_start: -7.0,
        bar_number: -1,
        signature_numerator: 7,
        signature_denominator: 8,
    })
}

#[test]
fn process_transport_matches_only_the_current_blocks_initial_event_under_rtsan() {
    let mut host = spawn_plugin(Some("transport"), SessionLayout::stereo());
    let note = Event::note_on(0, 1, 9, 60, 0.5);
    for frames in [1, 7, 64, 512] {
        // These planes and event slots cover only this regression's block shapes.
        let input = stereo(frames, 0.0);
        let mut output = stereo(frames, 0.0);
        let mut echoed = [note; 8];
        let mut cases = vec![
            (vec![], -1.0, 0.0),
            (vec![note, transport(0, true)], 137.5, 1.0),
            (vec![transport(0, false), note], 93.25, 1.0),
            // No retained snapshot, even immediately after a transport block.
            (vec![note], -1.0, 0.0),
        ];
        if frames > 1 {
            cases.extend([
                (
                    vec![transport(0, true), transport(frames as u32 - 1, false)],
                    137.5,
                    2.0,
                ),
                // A later update is not the state at sample zero.
                (vec![transport(1, false)], -1.0, 1.0),
            ]);
        }
        for reset in [false, true] {
            for (events, tempo, count) in &cases {
                if reset {
                    host.request_reset();
                }
                host.process_block(
                    frames,
                    &[&input[0], &input[1]],
                    &mut outs(&mut output),
                    events,
                    &mut echoed,
                    Duration::from_secs(2),
                )
                .expect("transport probe block");
                assert!(
                    output[0].iter().all(|sample| sample == tempo),
                    "process transport snapshot missing or stale: frames={frames}, reset={reset}, events={events:?}, output={:?}",
                    output[0]
                );
                assert!(
                    output[1].iter().all(|sample| sample == count),
                    "process transport differs from the initial event or timestamped updates changed: {:?}",
                    output[1]
                );
            }
        }
    }
}
