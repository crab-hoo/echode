//! What the sending side refuses before any byte is written, and the remap
//! that every sender can now pass with its descriptor.

use super::*;

/// The largest body a frame can carry: the length field also counts the type
/// and the request id.
const LARGEST_BODY: usize = MAX_MESSAGE - 12;

fn assert_too_long(error: &io::Error, message_type: u32) {
    let expected = ControlSendError::TooLong {
        message_type,
        length: MAX_MESSAGE + 1,
        maximum: MAX_MESSAGE,
    };
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "control message is longer than 64 MiB");
    assert_eq!(
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<ControlSendError>()),
        Some(&expected),
        "a refused frame must retain its typed size"
    );
}

#[test]
fn frames_past_the_receive_limit_are_refused_with_their_size() {
    // One byte past the largest body; the fixture is the limit, not a buffer.
    let body = vec![0; LARGEST_BODY + 1];
    let frame = frame_tagged(5, RESTORE, body.get(..LARGEST_BODY).unwrap()).unwrap();
    assert_eq!(
        frame.len(),
        4 + MAX_MESSAGE,
        "the largest body still frames"
    );
    drop(frame);
    assert_too_long(&frame_tagged(5, RESTORE, &body).unwrap_err(), RESTORE);

    let (mut sender, mut receiver) = Socket::pair().unwrap();
    let error = sender
        .send_tagged(9, &ControlMessage::State(body))
        .unwrap_err();
    assert_too_long(&error, STATE);
    assert!(
        receiver.try_recv().unwrap().is_none(),
        "a refused frame writes nothing"
    );
    sender.send_tagged(10, &ControlMessage::Dirty).unwrap();
    assert!(matches!(
        receiver.recv_tagged(Duration::from_secs(2)).unwrap(),
        (10, ControlMessage::Dirty)
    ));
}

#[test]
fn a_remap_sent_as_a_message_carries_its_descriptor() {
    // Three frames size this mapping fixture, not a product buffer.
    let layout = SessionLayout {
        max_frames: 3,
        input_channels: 1,
        output_channels: 1,
        input_events: 1,
        output_events: 1,
    };
    let region = crate::MappedRegion::create_shared(layout).unwrap();
    let message = ControlMessage::Remap {
        layout,
        fd: region.duplicate_fd().unwrap(),
    };
    let (mut sender, mut receiver) = Socket::pair().unwrap();
    sender.send_tagged(31, &message).unwrap();
    let (id, received) = receiver.recv_tagged(Duration::from_secs(2)).unwrap();
    assert_eq!(id, 31);
    let ControlMessage::Remap { layout: got, fd } = received else {
        panic!("expected a remap");
    };
    assert_eq!(got, layout);
    assert_eq!(crate::MappedRegion::from_fd(fd).unwrap().layout(), layout);
    let ControlMessage::Remap { fd: kept, .. } = &message else {
        unreachable!();
    };
    assert!(
        unsafe { libc::fcntl(kept.as_raw_fd(), libc::F_GETFD) } >= 0,
        "a borrowed message keeps the sender's descriptor open"
    );
}
