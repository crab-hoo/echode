use super::*;

#[test]
fn each_held_call_has_its_own_reached_release_and_finished_handshake() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut gate = ProcessGate::new(socket);
    let releases = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut byte = [0];
            peer.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [1]);
            peer.write_all(&[1]).unwrap();
            peer.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [2]);
        }
    });
    gate.hold();
    assert_eq!(gate.state.released.load(Ordering::Acquire), 1);
    gate.hold();
    assert_eq!(gate.state.released.load(Ordering::Acquire), 2);
    releases.join().unwrap();
}

#[test]
fn a_closed_test_peer_releases_processing_instead_of_leaving_a_spin() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut gate = ProcessGate::new(socket);
    let release = std::thread::spawn(move || {
        let mut byte = [0];
        peer.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [1]);
        // No release byte: the peer closes while processing is held.
    });
    gate.hold();
    assert!(gate.state.stopped.load(Ordering::Acquire));
    release.join().unwrap();
}

#[test]
fn deactivation_wakes_a_relay_waiting_for_the_test() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let gate = ProcessGate::new(socket);
    gate.state.requested.store(1, Ordering::Release);
    let mut byte = [0];
    peer.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [1]);
    drop(gate);
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}
