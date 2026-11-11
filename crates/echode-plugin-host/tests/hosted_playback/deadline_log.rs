//! A real helper that misses the engine's deadline is named in the log (DEV-8).

use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;

#[test]
fn contract_a_plugin_that_misses_the_engine_deadline_is_logged_by_its_node() {
    let capture = echode_log::testing::capture();
    let (project, track) = Project::new().add_track("held effect");
    let device = Id::generate();
    let project = project
        .add_clap_device(track, device, "dev.echode.test-gain")
        .unwrap();
    // The socket and the block length bound only this regression.
    let socket_path =
        PathBuf::from("/tmp").join(format!("echode-log-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket_path).unwrap();
    let route = PluginRoute {
        clap_id: "dev.echode.test-gain".into(),
        path: plugin_dylib(),
        instrument: false,
        fault: Some(format!("process-wait:{}", socket_path.display())),
        state: None,
    };
    let (mut control, mut audio) =
        playback_with_plugins(&project, 48_000, 64, &host_bin(), &[route]).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    control.replace(&project).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    audio.render_live_gated_until(
        &mut left,
        &mut right,
        Instant::now() + Duration::from_secs(5),
        true,
    );
    control.service();
    assert!(
        capture
            .with_field("device", &device.to_string().into())
            .is_empty(),
        "a block the helper finished logs nothing"
    );

    // The helper holds its second block, and the deadline has already passed.
    audio.render_live_gated_until(&mut left, &mut right, Instant::now(), true);
    let mut receipt = [0];
    peer.read_exact(&mut receipt).unwrap();
    assert_eq!(control.status().plugin_missed_blocks, 1);
    // The document thread's service names the node; the callback logged nothing.
    control.service();
    let records = capture.with_field("device", &device.to_string().into());
    let [record] = records.as_slice() else {
        panic!("one line names the missing node: {records:?}");
    };
    assert_eq!(record["level"], "warn");
    assert_eq!(record["message"], "a plugin did not finish its audio block");
    assert_eq!(record["fields"]["track"], track.to_string());
    assert_eq!(record["fields"]["plugin"], "dev.echode.test-gain");
    assert_eq!(record["fields"]["reason"], "deadline");
    assert_eq!(record["fields"]["sample"], 64);
    let late = capture
        .on_this_thread()
        .into_iter()
        .filter(|record| record["message"] == "an audio block finished after its deadline")
        .count();
    assert_eq!(late, 1, "the block past its deadline is logged too");
    let _ = peer.write_all(&[1]);
    let _ = std::fs::remove_file(&socket_path);
}
