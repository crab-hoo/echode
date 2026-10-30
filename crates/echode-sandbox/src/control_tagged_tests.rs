use super::*;

#[test]
fn tagged_messages_and_descriptors_keep_their_own_ids() {
    let (mut parent, mut child) = Socket::pair().unwrap();
    // Seven frames exercise this mapping fixture, not a product buffer limit.
    let layout = SessionLayout {
        max_frames: 7,
        input_channels: 0,
        output_channels: 1,
        input_events: 1,
        output_events: 1,
    };
    let region = crate::MappedRegion::create_shared(layout).unwrap();
    parent
        .send_tagged(u64::MAX, &ControlMessage::SaveRequest)
        .unwrap();
    parent
        .send_remap_tagged(42, layout, region.duplicate_fd().unwrap())
        .unwrap();
    parent
        .send_tagged(43, &ControlMessage::Restore(vec![128]))
        .unwrap();
    assert!(matches!(
        child.recv_tagged(Duration::from_secs(2)).unwrap(),
        (u64::MAX, ControlMessage::SaveRequest)
    ));
    let (id, message) = child.recv_tagged(Duration::from_secs(2)).unwrap();
    assert_eq!(id, 42);
    let ControlMessage::Remap { layout: got, fd } = message else {
        panic!("missing remap")
    };
    assert_eq!(got, layout);
    assert_eq!(crate::MappedRegion::from_fd(fd).unwrap().layout(), layout);
    assert!(
        matches!(child.recv_tagged(Duration::from_secs(2)).unwrap(), (43, ControlMessage::Restore(bytes)) if bytes == [128])
    );
    child.send(&ControlMessage::Dirty).unwrap();
    assert!(matches!(
        parent.recv_tagged(Duration::from_secs(2)).unwrap(),
        (0, ControlMessage::Dirty)
    ));
}

#[test]
fn short_headers_are_rejected_and_partial_frames_wait_for_completion() {
    let (mut socket, _peer) = Socket::pair().unwrap();
    socket.incoming = 11u32.to_le_bytes().to_vec();
    assert_eq!(
        socket.parse_one().unwrap_err().kind(),
        ErrorKind::InvalidData
    );
    let frame = frame_tagged(17, RESTORE, &[128]).unwrap();
    for split in 0..frame.len() {
        socket.incoming = frame[..split].to_vec();
        assert!(
            socket.parse_one().unwrap().is_none(),
            "partial frame at {split}"
        );
    }
    socket.incoming = frame;
    assert!(
        matches!(socket.parse_one().unwrap(), Some((17, ControlMessage::Restore(bytes))) if bytes == [128])
    );
}
