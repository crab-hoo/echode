//! Held helper startup must not hold the document's control turns.
use super::*;
use std::io::{Read, Write};
use std::sync::mpsc;

struct Release(std::os::unix::net::UnixStream);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.0.write_all(&[1]);
    }
}

pub(super) fn initializer_peer(
    peer: std::os::unix::net::UnixStream,
) -> std::os::unix::net::UnixStream {
    // Darwin inherits O_NONBLOCK from the listener; a read timeout does not clear it.
    peer.set_nonblocking(false).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    peer
}

#[test]
fn accepted_initializer_peer_is_blocking_before_the_handshake_arrives() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    let socket = Path::new("/private/tmp").join(format!("ep-{}.sock", Id::generate().raw()));
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let _client = UnixStream::connect(&socket).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let peer = initializer_peer(peer);
    std::fs::remove_file(socket).unwrap();
    // SAFETY: the stream owns a live descriptor; F_GETFL takes no extra argument.
    let flags = unsafe { libc::fcntl(peer.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_eq!(
        flags & libc::O_NONBLOCK,
        0,
        "initializer peer must block for the handshake instead of returning WouldBlock"
    );
}

#[test]
fn contract_background_graph_keeps_control_turns_and_audio_under_rtsan() {
    // These buffers bound this fixture only, not supported project geometry.
    let frames = 64;
    let (base, track) = Project::new().add_track("continuous");
    let existing = Id::generate();
    let base = base
        .add_clap_device(track, existing, "dev.echode.test-gain")
        .unwrap();
    let path = plugin_dylib();
    let (mut control, mut audio) =
        playback_with_plugins(&base, 48_000, frames, &host_bin(), &[effect_route(&path)]).unwrap();
    control.set_clip_loader(constant_clips(track.raw(), 0.125, frames * 1024));
    control.replace(&base).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    render_content(&mut audio, &mut left, &mut right);
    let before = control.status().sample;
    let socket = Path::new("/private/tmp").join(format!("eb-{}.sock", Id::generate().raw()));
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (changed, added) = base.add_track("initializing");
    let added_device = Id::generate();
    let changed = changed
        .add_clap_device(added, added_device, "dev.echode.test-gain")
        .unwrap();
    let mut waiting = effect_route(&path);
    waiting.fault = Some(format!("prepare-wait:{}", socket.display()));
    let (done, completed) = mpsc::channel();
    let (finish, finishing) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        control.prepare_in_background();
        control.set_routes(&host_bin(), &[effect_route(&path), waiting]);
        control.replace(&changed).unwrap();
        done.send("replace").unwrap();
        // These turns run while the new helper's initialization is held.
        control.service();
        assert!(control.device_control_handle(existing.raw()).is_ok());
        assert!(control.device_applied_state(existing.raw()).is_ok());
        assert!(control.status().pending_plan);
        // Undo/Redo and a later deletion coalesce without waiting for startup.
        control.replace(&base).unwrap();
        control.replace(&changed).unwrap();
        control.replace(&base).unwrap();
        control.command(PlaybackCommand::Pause).unwrap();
        done.send("turns").unwrap();
        finishing.recv().unwrap();
        control
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut peer = loop {
        match listener.accept() {
            Ok((peer, _)) => break initializer_peer(peer),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("initializer handshake: {error}"),
        }
    };
    let mut reached = [0];
    peer.read_exact(&mut reached).unwrap();
    let release = Release(peer);
    // A hung-worker guard, not a latency/performance threshold.
    assert_eq!(
        completed
            .recv_timeout(Duration::from_secs(5))
            .expect("replace held the control turn"),
        "replace"
    );
    assert_eq!(
        completed
            .recv_timeout(Duration::from_secs(5))
            .expect("later control turns blocked"),
        "turns"
    );
    // Pause is queued independently of the prepared bank.
    render_content(&mut audio, &mut left, &mut right);
    let paused = before;
    finish.send(()).unwrap();
    let mut control = worker.join().unwrap();
    assert!(!control.status().playing);
    assert_eq!(control.status().sample, paused);
    control.command(PlaybackCommand::Play).unwrap();
    let expected = 0.25 * std::f32::consts::FRAC_1_SQRT_2;
    for _ in 0..8 {
        control.service();
        render_content(&mut audio, &mut left, &mut right);
        assert!(
            left.iter()
                .chain(&right)
                .all(|v| (*v - expected).abs() < 1.0e-7)
        );
    }
    assert_eq!(control.status().sample, paused + (frames * 8) as i64);
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(10);
    while control.status().pending_plan {
        control.service();
        assert!(control.take_preparation_error().is_none());
        render_content(&mut audio, &mut left, &mut right);
        assert!(Instant::now() < deadline, "replacement never applied");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        matches!(
            control.device_control_handle(added_device.raw()),
            Err(echode_engine::PlaybackError::NoRunningPlugin)
        ),
        "deleted helper remained in the catalog"
    );
    assert!(control.device_control_handle(existing.raw()).is_ok());
    assert_eq!(control.status().graph_unavailable_blocks, 0);
    assert_eq!(control.status().plugin_missed_blocks, 0);
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn state_undo_during_startup_keeps_the_existing_helper_and_latest_recorded_bytes() {
    // The block length bounds this state/lifetime fixture only.
    let frames = 64;
    let (base, track) = Project::new().add_track("parameters");
    let device = Id::generate();
    let base = base
        .add_clap_device(track, device, "dev.echode.test-params")
        .unwrap();
    let mut defaults = Vec::from(1.0f64.to_le_bytes());
    defaults.extend(0.0f64.to_le_bytes());
    let (mut control, mut audio) = playback_with_plugins(
        &base,
        48_000,
        frames,
        &host_bin(),
        &[device_params::params_route(Some(defaults.clone()))],
    )
    .unwrap();
    let original = control.device_control_handle(device.raw()).unwrap();
    control.prepare_in_background();
    let socket = Path::new("/private/tmp").join(format!("es-{}.sock", Id::generate().raw()));
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let added = Id::generate();
    let changed = base
        .add_clap_device(track, added, "dev.echode.test-gain")
        .unwrap()
        .reorder_device(added, Some(device))
        .unwrap();
    let mut waiting = effect_route(&plugin_dylib());
    waiting.fault = Some(format!("prepare-wait:{}", socket.display()));
    control.set_routes(
        &host_bin(),
        &[waiting.clone(), device_params::params_route(Some(defaults))],
    );
    control.replace(&changed).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut peer = loop {
        match listener.accept() {
            Ok((peer, _)) => break initializer_peer(peer),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("initializer handshake: {error}"),
        }
    };
    peer.read_exact(&mut [0]).unwrap();
    let release = Release(peer);
    // The old parameter helper is after the held initializer in the new chain.
    let mut edited = Vec::from(1.5f64.to_le_bytes());
    edited.extend(0.0f64.to_le_bytes());
    control.restore_device_state(device.raw(), &edited).unwrap();
    control.set_routes(
        &host_bin(),
        &[waiting, device_params::params_route(Some(edited.clone()))],
    );
    control.replace(&changed).unwrap();
    control
        .command(PlaybackCommand::SetLoop {
            start: 0,
            end: 48_000,
        })
        .unwrap();
    drop(release);
    let mut left = vec![0.0; frames];
    let mut right = left.clone();
    while control.status().pending_plan {
        control.service();
        render_content(&mut audio, &mut left, &mut right);
        assert!(control.take_preparation_error().is_none());
        assert!(Instant::now() < deadline, "latest state never applied");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        control.device_applied_state(device.raw()).unwrap(),
        Some(edited)
    );
    assert_eq!(
        original.parameters().expect("existing helper was replaced")[0].value,
        1.5
    );
    std::fs::remove_file(socket).unwrap();
}
