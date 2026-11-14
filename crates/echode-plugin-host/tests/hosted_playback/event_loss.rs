//! Recover current notes when a helper could not receive a block (RTA-04).

use super::*;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

struct Gate {
    peer: UnixStream,
    path: PathBuf,
}

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Copy)]
enum LostNote {
    Off,
    On,
    Held,
    OnBeforeLoop,
}

fn recovery(lost: LostNote, downstream: bool, change_plan: bool, event_block: usize, rate: u32) {
    recovery_with_late_chase(lost, downstream, change_plan, event_block, rate, false);
}

fn recovery_with_late_chase(
    lost: LostNote,
    downstream: bool,
    change_plan: bool,
    event_block: usize,
    rate: u32,
    late_chase: bool,
) {
    let (project, track) = Project::new().add_track("held instrument");
    let quarter = Tick::quarters(1).unwrap();
    let boundary =
        usize::try_from(tick_to_sample(quarter, project.tempo(), rate).unwrap()).unwrap();
    // The block lengths and note lengths describe only this regression. The
    // note edge falls within `event_block`, not on a block boundary.
    let frames = boundary * 2 / (event_block * 2 - 1);
    assert!(((event_block - 1) * frames..event_block * frames).contains(&boundary));
    let (start, length) = match lost {
        LostNote::Off => (Tick::new(0), quarter),
        LostNote::On | LostNote::OnBeforeLoop => (quarter, Tick::quarters(100).unwrap()),
        LostNote::Held => (Tick::new(0), Tick::quarters(100).unwrap()),
    };
    let mut project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start,
                length,
            },
        )
        .unwrap()
        .0
        .add_clap_device(track, Id::generate(), "dev.echode.test-instrument")
        .unwrap();
    if downstream {
        project = project
            .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
            .unwrap();
    }
    let socket_path =
        PathBuf::from("/tmp").join(format!("echode-rta04-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let routes = [
        PluginRoute {
            clap_id: "dev.echode.test-instrument".into(),
            path: plugin_dylib(),
            instrument: true,
            fault: Some(format!(
                "{}:{}",
                if late_chase {
                    "onset-wait-twice"
                } else {
                    "onset-wait"
                },
                socket_path.display()
            )),
            state: None,
        },
        PluginRoute {
            clap_id: "dev.echode.test-gain".into(),
            path: plugin_dylib(),
            instrument: false,
            fault: Some("onset".into()),
            state: None,
        },
    ];
    let (mut control, mut audio) =
        playback_with_plugins(&project, rate, frames, &host_bin(), &routes).unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut gate = Gate {
        peer,
        path: socket_path,
    };
    if matches!(lost, LostNote::OnBeforeLoop) {
        control
            .command(PlaybackCommand::SetLoop {
                start: 0,
                end: (5 * frames) as i64,
            })
            .unwrap();
    }
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    let patient = || Instant::now() + Duration::from_secs(5);

    audio.render_live_gated_until(&mut left, &mut right, patient(), true);
    if matches!(lost, LostNote::Off | LostNote::Held) {
        assert!(left.iter().any(|sample| sample.abs() > 0.1));
    }
    assert_eq!(control.status().plugin_missed_blocks, 0);

    // Block two is held; block three is published behind it. Four and five
    // cannot be published. A downstream helper receives none of these blocks.
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    let mut receipt = [0];
    gate.peer.read_exact(&mut receipt).unwrap();
    for _ in 3..=5 {
        audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    }
    assert_eq!(control.status().plugin_missed_blocks, 4);
    if change_plan {
        project = project.add_track("unrelated edit").0;
        control.replace(&project).unwrap();
    }

    gate.peer.write_all(&[1]).unwrap();
    gate.peer.read_exact(&mut receipt).unwrap();
    let _gate = if late_chase {
        delay_chase(gate, &mut audio, &control, &mut left, &mut right)
    } else {
        gate
    };
    // Wait for an actual recovered block, not a fixed number of fast callbacks.
    // A busy submission returns immediately even with a patient deadline.
    let until = patient();
    loop {
        let misses = control.status().plugin_missed_blocks;
        audio.render_live_gated_until(&mut left, &mut right, until, true);
        if control.status().plugin_missed_blocks == misses {
            break;
        }
        assert!(Instant::now() < until, "the helper never caught up");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!control.status().failed);
    match lost {
        LostNote::Off | LostNote::OnBeforeLoop => assert!(
            left.iter().chain(&right).all(|sample| *sample == 0.0),
            "a note whose off was not delivered is still sounding after recovery"
        ),
        LostNote::On | LostNote::Held => assert!(
            left.iter().any(|sample| sample.abs() > 0.1),
            "a note whose on was not delivered was not chased after recovery"
        ),
    }
    // Successful subsequent blocks must not reset or chase the same note again.
    let previous = *left.last().unwrap();
    let slope = previous - left[left.len() - 2];
    let misses = control.status().plugin_missed_blocks;
    audio.render_live_gated_until(&mut left, &mut right, patient(), true);
    assert_eq!(control.status().plugin_missed_blocks, misses);
    match lost {
        LostNote::Off | LostNote::OnBeforeLoop => {
            assert!(left.iter().chain(&right).all(|sample| *sample == 0.0))
        }
        LostNote::On | LostNote::Held => {
            // The onset fixture has a 64-sample triangle and a 32-sample attack.
            // A repeated reset/chase would instead start the next block at zero.
            let expected = previous
                + if previous.abs() == 0.25 {
                    -slope
                } else {
                    slope
                };
            assert_level(left[0], expected);
        }
    }
}

// Hold the recovery chase past its deadline as well. Later successful blocks
// must acknowledge that published chase without striking its notes again.
fn delay_chase(
    mut gate: Gate,
    audio: &mut echode_engine::PlaybackAudio,
    control: &echode_engine::PlaybackControl,
    left: &mut [f32],
    right: &mut [f32],
) -> Gate {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut receipt = [0];
        gate.peer.read_exact(&mut receipt).unwrap();
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        gate.peer.write_all(&[1]).unwrap();
        gate.peer.read_exact(&mut receipt).unwrap();
        gate
    });
    let until = Instant::now() + Duration::from_secs(5);
    while entered_rx.try_recv().is_err() {
        audio.render_live_gated_until(left, right, Instant::now(), true);
        assert!(
            Instant::now() < until,
            "the recovery chase was not published"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(control.status().plugin_missed_blocks >= 5);
    release_tx.send(()).unwrap();
    worker.join().unwrap()
}

#[test]
fn audit_rta04_a_late_recovery_chase_is_not_duplicated_after_a_later_answer() {
    recovery_with_late_chase(LostNote::On, false, false, 4, 48_000, true);
}

#[test]
fn audit_rta04_a_note_off_in_a_block_the_busy_helper_refused_stops_the_note() {
    for rate in [44_100, 48_000] {
        recovery(LostNote::Off, false, false, 4, rate);
    }
}

#[test]
fn audit_rta04_a_note_on_in_a_refused_block_is_chased_once() {
    recovery(LostNote::On, false, false, 4, 48_000);
}

#[test]
fn audit_rta04_a_missed_block_without_events_does_not_reset_held_notes() {
    recovery(LostNote::Held, true, false, 4, 48_000);
}

#[test]
fn audit_rta04_recovery_at_a_loop_restart_does_not_restrike_previous_voices() {
    recovery(LostNote::OnBeforeLoop, false, false, 4, 48_000);
}

#[test]
fn audit_rta04_downstream_recovers_even_when_the_upstream_received_the_note_off() {
    recovery(LostNote::Off, true, false, 3, 48_000);
}

#[test]
fn audit_rta04_note_recovery_survives_a_plan_change() {
    for lost in [LostNote::Off, LostNote::On] {
        recovery(lost, true, true, 4, 48_000);
    }
}
