//! Notes of a block the callback could not hand to a busy helper (RTA-04).

use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;

fn live_until(
    audio: &mut echode_engine::PlaybackAudio,
    left: &mut [f32],
    right: &mut [f32],
    deadline: Instant,
) {
    audio.render_live_gated_until(left, right, deadline, true);
}

/// The helper holds one `process` call for longer than two blocks. The
/// callback gives up on that block and on the next one, which the shared
/// region still takes. The third block finds both slots in use: it is
/// reported as a failed submission and its events are not kept for a later
/// block. Here that block carries the note-off, so the plugin holds the note
/// after the helper has caught up.
#[test]
#[ignore = "RTA-04: a note-off in a block a busy helper could not take never reaches the plugin"]
fn audit_rta04_a_note_off_in_a_block_the_busy_helper_refused_still_reaches_the_plugin() {
    let rate = 48_000;
    let (project, track) = Project::new().add_track("held instrument");
    let length = Tick::quarters(1).unwrap();
    let project = project
        .insert_note(
            track,
            NoteDraft {
                pitch: 60,
                velocity: 100,
                start: Tick::new(0),
                length,
            },
        )
        .unwrap()
        .0
        .add_clap_device(track, Id::generate(), "dev.echode.test-gain")
        .unwrap();
    let off = usize::try_from(tick_to_sample(length, project.tempo(), rate).unwrap()).unwrap();
    // Blocks long enough that the note ends inside the fourth one. The length
    // bounds only this regression.
    let frames = off * 2 / 7;
    assert!(
        (3 * frames..4 * frames).contains(&off),
        "the note-off falls in the fourth block"
    );
    let socket_path =
        PathBuf::from("/tmp").join(format!("echode-rta04-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: true,
        fault: Some(format!("count-wait:{}", socket_path.display())),
        state: None,
    };
    let (mut control, mut audio) =
        playback_with_plugins(&project, rate, frames, &host_bin(), &[route]).unwrap();
    let (mut gate, _) = listener.accept().unwrap();
    gate.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    let patient = || Instant::now() + Duration::from_secs(5);

    // The first block carries the note-on and is heard.
    live_until(&mut audio, &mut left, &mut right, patient());
    assert_level(*left.last().unwrap(), 1.0);

    // The helper holds its second block past the deadline; the callback
    // gives up on it and on the third, which waits behind it.
    live_until(&mut audio, &mut left, &mut right, Instant::now());
    let mut receipt = [0];
    gate.read_exact(&mut receipt).unwrap();
    live_until(&mut audio, &mut left, &mut right, Instant::now());
    // The fourth block holds the note-off and finds no free slot.
    live_until(&mut audio, &mut left, &mut right, Instant::now());
    assert_eq!(
        control.status().plugin_missed_blocks,
        3,
        "three blocks in a row were missed"
    );

    // The helper catches up with every block it was given.
    gate.write_all(&[1]).unwrap();
    gate.read_exact(&mut receipt).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    for _ in 0..4 {
        live_until(&mut audio, &mut left, &mut right, patient());
    }
    assert!(!control.status().failed);
    // One note-on and one note-off: the counter writes 1 + 16.
    assert_level(*left.last().unwrap(), 17.0);
    let _ = std::fs::remove_file(&socket_path);
}
