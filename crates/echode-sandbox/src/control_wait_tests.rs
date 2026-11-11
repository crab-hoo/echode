//! How a send waits while the peer has no room: it reads what the peer queued,
//! and a deadline ends it without leaving the peer a message (R9, ADR 0319).

use super::*;

/// Twice both 1 MiB socket buffers, so neither end can queue the whole
/// message. It sizes these fixtures only.
const PAST_BUFFERS: usize = 4 * 1024 * 1024;

/// Each end sends a state past both buffers before it reads, then reads the
/// other's, as the host and the helper do. Each used to wait in its send for
/// the other to read, for good.
#[test]
fn two_ends_that_send_past_the_buffers_at_once_both_finish() {
    let (left, right) = Socket::pair().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    for (mut socket, id, message) in [
        (left, 1, ControlMessage::State(vec![1; PAST_BUFFERS])),
        (right, 2, ControlMessage::Restore(vec![2; PAST_BUFFERS])),
    ] {
        let done = done.clone();
        std::thread::spawn(move || {
            let sent = socket
                .send_tagged(id, &message)
                .map_err(|error| error.kind());
            let received = socket
                .recv_tagged(Duration::from_secs(5))
                .map(|(id, message)| {
                    let bytes = match message {
                        ControlMessage::State(bytes) | ControlMessage::Restore(bytes) => bytes,
                        _ => Vec::new(),
                    };
                    (id, bytes.len(), bytes.first().copied())
                });
            let _ = done.send((id, sent, received.map_err(|error| error.kind())));
        });
    }
    let mut seen = Vec::new();
    for _ in 0..2 {
        seen.push(
            finished
                .recv_timeout(Duration::from_secs(10))
                .expect("both sends must finish while each reads the other"),
        );
    }
    seen.sort_by_key(|(id, ..)| *id);
    assert_eq!(
        seen,
        [
            (1, Ok(()), Ok((2, PAST_BUFFERS, Some(2)))),
            (2, Ok(()), Ok((1, PAST_BUFFERS, Some(1)))),
        ]
    );
}

fn send_cause(error: &io::Error) -> (ErrorKind, Option<ControlSendError>) {
    (
        error.kind(),
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<ControlSendError>())
            .copied(),
    )
}

/// A peer that never reads used to hold a sender past the buffers for good.
#[test]
fn a_send_to_a_peer_that_never_reads_is_cut_off_at_its_deadline() {
    let (mut sender, mut receiver) = Socket::pair().unwrap();
    let start = Instant::now();
    let error = sender
        .send_tagged_until(
            6,
            &ControlMessage::Restore(vec![6; PAST_BUFFERS]),
            start + Duration::from_millis(200),
        )
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(
        (Duration::from_millis(200)..Duration::from_secs(2)).contains(&elapsed),
        "the send ends at its deadline: {elapsed:?}"
    );
    let (
        ErrorKind::TimedOut,
        Some(ControlSendError::Abandoned {
            message_type,
            sent,
            total,
        }),
    ) = send_cause(&error)
    else {
        panic!("a cut-off send must say how far it got: {error:?}");
    };
    assert_eq!((message_type, total), (RESTORE, 16 + PAST_BUFFERS));
    assert!(0 < sent && sent < total, "{sent} of {total}");

    // Nothing more is sent, with or without a deadline.
    for later in [
        sender.send_tagged(7, &ControlMessage::Dirty),
        sender.send_tagged_until(
            8,
            &ControlMessage::Dirty,
            Instant::now() + Duration::from_secs(1),
        ),
    ] {
        assert_eq!(
            send_cause(&later.unwrap_err()),
            (ErrorKind::BrokenPipe, Some(ControlSendError::Closed))
        );
    }
    // The peer gets the cut-off bytes, then end of file: never a whole message.
    let error = loop {
        match receiver.try_recv_tagged() {
            Ok(None) => std::thread::yield_now(),
            Ok(Some(message)) => panic!("a cut-off message was delivered: {message:?}"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
    assert_eq!(receiver.incoming.len(), sent);
}

/// Darwin takes a write of at most 2,048 bytes (`SO_SNDLOWAT`) whole or not
/// at all, so filling the buffers with such messages ends on a message that
/// found no room. 2,000 bytes sizes this fixture only.
#[test]
fn a_send_that_finds_no_room_by_its_deadline_leaves_the_stream_intact() {
    let (mut sender, mut receiver) = Socket::pair().unwrap();
    let filler = ControlMessage::State(vec![4; 2_000 - 16]);
    let mut queued = 0usize;
    let error = loop {
        match sender.send_tagged_until(9, &filler, Instant::now()) {
            Ok(()) => queued += 1,
            Err(error) => break error,
        }
        assert!(queued < 10_000, "the socket buffers never filled");
    };
    assert_eq!(
        send_cause(&error),
        (
            ErrorKind::TimedOut,
            Some(ControlSendError::Unsent {
                message_type: STATE,
                total: 2_000
            })
        )
    );
    let mut received = 0usize;
    while let Some((id, message)) = receiver.try_recv_tagged().unwrap() {
        assert!(
            matches!((id, message), (9, ControlMessage::State(bytes)) if bytes.len() == 2_000 - 16)
        );
        received += 1;
    }
    assert_eq!(received, queued, "only whole messages were queued");
    sender
        .send_tagged_until(
            10,
            &ControlMessage::Dirty,
            Instant::now() + Duration::from_secs(2),
        )
        .expect("an unsent message leaves later sends working");
    assert!(matches!(
        receiver.recv_tagged(Duration::from_secs(2)).unwrap(),
        (10, ControlMessage::Dirty)
    ));
}

/// The peer queued two remaps and a state before this end sent. Reading them
/// during the send takes each message before the next read, so the second
/// descriptor is not refused as arriving before the first was consumed.
#[test]
fn messages_read_during_a_send_keep_their_order_and_descriptors() {
    // One frame and one channel size this mapping fixture, not a product buffer.
    let layout = SessionLayout {
        max_frames: 1,
        input_channels: 0,
        output_channels: 1,
        input_events: 1,
        output_events: 1,
    };
    let region = crate::MappedRegion::create_shared(layout).unwrap();
    let (mut sender, mut peer) = Socket::pair().unwrap();
    for id in [1, 2] {
        peer.send_remap_tagged(id, layout, region.duplicate_fd().unwrap())
            .unwrap();
    }
    peer.send_tagged(3, &ControlMessage::State(vec![3; 3]))
        .unwrap();
    let reader = std::thread::spawn(move || {
        let (id, message) = peer.recv_tagged(Duration::from_secs(10)).unwrap();
        (
            id,
            matches!(message, ControlMessage::State(bytes) if bytes.len() == PAST_BUFFERS),
        )
    });
    sender
        .send_tagged(9, &ControlMessage::State(vec![9; PAST_BUFFERS]))
        .unwrap();
    assert_eq!(
        sender.ready.len(),
        3,
        "the peer's messages were read during the send"
    );
    assert!(sender.wait_readable(Duration::ZERO).unwrap());
    for expected in [1, 2] {
        let Some((id, ControlMessage::Remap { layout: got, fd })) =
            sender.try_recv_tagged().unwrap()
        else {
            panic!("expected remap {expected}");
        };
        assert_eq!((id, got), (expected, layout));
        assert_eq!(crate::MappedRegion::from_fd(fd).unwrap().layout(), layout);
    }
    assert!(matches!(
        sender.try_recv_tagged().unwrap(),
        Some((3, ControlMessage::State(bytes))) if bytes == [3, 3, 3]
    ));
    assert_eq!(reader.join().unwrap(), (9, true));
}

/// An error met while reading during a send is returned where the receive
/// path would have met it: after the messages before it.
#[test]
fn an_error_read_during_a_send_follows_the_messages_before_it() {
    let (mut sender, mut peer) = Socket::pair().unwrap();
    send_bytes(peer.as_raw_fd(), &frame_tagged(0, DIRTY, b""), None).unwrap();
    send_bytes(peer.as_raw_fd(), &frame_tagged(4, 99, b""), None).unwrap();
    let reader = std::thread::spawn(move || {
        let (id, message) = peer.recv_tagged(Duration::from_secs(10)).unwrap();
        (
            id,
            matches!(message, ControlMessage::State(bytes) if bytes.len() == PAST_BUFFERS),
        )
    });
    sender
        .send_tagged(5, &ControlMessage::State(vec![5; PAST_BUFFERS]))
        .unwrap();
    assert!(matches!(
        sender.try_recv_tagged().unwrap(),
        Some((0, ControlMessage::Dirty))
    ));
    let error = sender.try_recv_tagged().unwrap_err();
    assert_eq!(
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<ControlDecodeError>()),
        Some(&ControlDecodeError::UnknownMessageType(99))
    );
    assert_eq!(reader.join().unwrap(), (5, true));
}
