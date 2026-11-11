//! How a message is split into frames on send and joined on receive
//! (ADR 0318), and the remap that every sender passes with its descriptor.

use super::*;
use crate::control_transport::{MAX_BODY, MAX_FRAME, PART};
use std::io::Read;

/// A 4-byte frame body splits these fixtures into several frames. It sizes
/// the fixtures only; the product splits at [`MAX_BODY`].
const SMALL_BODY: usize = 4;

/// What one split send wrote, read raw from the other end.
fn sent_raw(id: u64, message: &ControlMessage, max_body: usize) -> Vec<u8> {
    let (mut sender, receiver) = Socket::pair().unwrap();
    sender.send_split(id, message, max_body, None).unwrap();
    drop(sender);
    let mut raw = std::os::unix::net::UnixStream::from(receiver.fd);
    let mut bytes = Vec::new();
    raw.read_to_end(&mut bytes).unwrap();
    bytes
}

/// Each frame's length field, type and request id, in wire order.
fn frame_headers(mut bytes: &[u8]) -> Vec<(usize, u32, u64)> {
    let mut headers = Vec::new();
    while let Some((len, rest)) = bytes.split_first_chunk::<4>() {
        let len = u32::from_le_bytes(*len) as usize;
        let (typ, rest) = rest.split_first_chunk::<4>().expect("a type");
        let (id, _) = rest.split_first_chunk::<8>().expect("a request id");
        headers.push((len, u32::from_le_bytes(*typ), u64::from_le_bytes(*id)));
        bytes = bytes.get(4 + len..).expect("a whole frame");
    }
    headers
}

fn assert_interleaved(error: &io::Error, expected: u64, actual: u64) {
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "control message parts were interleaved");
    assert_eq!(
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<ControlDecodeError>()),
        Some(&ControlDecodeError::InterleavedPart { expected, actual }),
        "an interleaved part must retain both request ids"
    );
}

#[test]
fn the_largest_frame_body_is_the_largest_frame_a_receiver_accepts() {
    assert_eq!(12 + MAX_BODY, MAX_FRAME);
}

#[test]
fn bodies_past_one_frame_cross_as_parts_and_join_into_one_message() {
    for len in [0usize, 1, 3, 4, 5, 8, 9, 13] {
        let body: Vec<u8> = (1..=len as u8).collect();
        let message = ControlMessage::State(body.clone());
        // Every frame before the last is full; the last carries the rest.
        let parts = len.saturating_sub(1) / SMALL_BODY;
        let mut expected = vec![(12 + SMALL_BODY, PART, 7); parts];
        expected.push((12 + len - parts * SMALL_BODY, STATE, 7));
        assert_eq!(
            frame_headers(&sent_raw(7, &message, SMALL_BODY)),
            expected,
            "{len} bytes"
        );

        let (mut sender, mut receiver) = Socket::pair().unwrap();
        sender.send_tagged(0, &ControlMessage::Dirty).unwrap();
        sender.send_split(7, &message, SMALL_BODY, None).unwrap();
        sender.send_tagged(0, &ControlMessage::Dirty).unwrap();
        let mut received = Vec::new();
        for _ in 0..3 {
            received.push(receiver.recv_tagged(Duration::from_secs(2)).unwrap());
        }
        assert!(
            matches!(
                received.as_slice(),
                [
                    (0, ControlMessage::Dirty),
                    (7, ControlMessage::State(bytes)),
                    (0, ControlMessage::Dirty),
                ] if *bytes == body
            ),
            "{len} bytes: {received:?}"
        );
    }
}

#[test]
fn every_message_type_joins_the_same_way() {
    let messages = [
        ControlMessage::Restore(b"restored bytes".to_vec()),
        ControlMessage::GuiShow("A window title".into()),
        ControlMessage::SetParams(vec![
            ParamChange { id: 4, value: 0.5 },
            ParamChange { id: 9, value: -1.0 },
        ]),
        ControlMessage::StateRejected(Refusal::Busy),
    ];
    for message in &messages {
        let (mut sender, mut receiver) = Socket::pair().unwrap();
        sender.send_split(11, message, SMALL_BODY, None).unwrap();
        let (id, received) = receiver.recv_tagged(Duration::from_secs(2)).unwrap();
        assert_eq!(id, 11);
        assert_eq!(format!("{received:?}"), format!("{message:?}"));
    }
}

#[test]
fn a_frame_of_another_request_between_parts_is_refused_with_both_ids() {
    for (other, typ) in [(0, DIRTY), (6, PART), (6, STATE)] {
        let (sender, mut receiver) = Socket::pair().unwrap();
        send_bytes(sender.as_raw_fd(), &frame_tagged(5, PART, b"ab"), None).unwrap();
        send_bytes(sender.as_raw_fd(), &frame_tagged(other, typ, b""), None).unwrap();
        let error = receiver.recv_tagged(Duration::from_secs(2)).unwrap_err();
        assert_interleaved(&error, 5, other);
        // The joined bytes were dropped with the refusal.
        send_bytes(sender.as_raw_fd(), &frame_tagged(5, STATE, b"cd"), None).unwrap();
        assert!(matches!(
            receiver.recv_tagged(Duration::from_secs(2)).unwrap(),
            (5, ControlMessage::State(bytes)) if bytes == b"cd"
        ));
    }
}

#[test]
fn a_message_waits_for_the_frame_of_its_own_type() {
    let (sender, mut receiver) = Socket::pair().unwrap();
    send_bytes(sender.as_raw_fd(), &frame_tagged(5, PART, b"ab"), None).unwrap();
    send_bytes(sender.as_raw_fd(), &frame_tagged(5, PART, b"cd"), None).unwrap();
    for _ in 0..3 {
        assert!(receiver.try_recv_tagged().unwrap().is_none());
    }
    send_bytes(sender.as_raw_fd(), &frame_tagged(5, RESTORE, b"e"), None).unwrap();
    assert!(matches!(
        receiver.recv_tagged(Duration::from_secs(2)).unwrap(),
        (5, ControlMessage::Restore(bytes)) if bytes == b"abcde"
    ));
    // A peer that ends inside a message ends the stream, not with a message.
    send_bytes(sender.as_raw_fd(), &frame_tagged(8, PART, b"ab"), None).unwrap();
    drop(sender);
    assert_eq!(
        receiver
            .recv_tagged(Duration::from_secs(2))
            .unwrap_err()
            .kind(),
        ErrorKind::UnexpectedEof
    );
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

#[test]
fn a_split_remap_passes_its_descriptor_with_the_frame_of_its_type() {
    // One frame and one channel size this mapping fixture, not a product buffer.
    let layout = SessionLayout {
        max_frames: 1,
        input_channels: 0,
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
    // The 20-byte layout crosses as five parts and the remap frame.
    sender.send_split(3, &message, SMALL_BODY, None).unwrap();
    let ControlMessage::Remap { layout: got, fd } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected a remap");
    };
    assert_eq!(got, layout);
    assert_eq!(crate::MappedRegion::from_fd(fd).unwrap().layout(), layout);
}

/// A background save is collected by polling. One poll used to read one
/// 8 KiB chunk, so an 8 MiB save took 1,026 polls (24 s at the document's
/// 20 ms) while the helper waited in its send (ADR 0318).
#[test]
fn one_poll_takes_every_byte_already_queued() {
    // Half the 1 MiB socket buffers, so the whole message is queued before
    // any read. It sizes this fixture only.
    let body = vec![3u8; 512 * 1024];
    let message = ControlMessage::State(body.clone());
    let (mut sender, mut receiver) = Socket::pair().unwrap();
    let (sent, done) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let result = sender.send_tagged(4, &message);
        sent.send(result.is_ok()).unwrap();
        sender
    });
    assert_eq!(
        done.recv_timeout(Duration::from_secs(2)),
        Ok(true),
        "a message within the socket buffers is queued without a reader"
    );
    assert!(matches!(
        receiver.try_recv_tagged().unwrap(),
        Some((4, ControlMessage::State(bytes))) if bytes == body
    ));
    drop(writer.join().unwrap());
}

#[test]
fn one_poll_stops_after_its_budget_and_the_next_continues() {
    // Four 64 KiB parts and a final frame: 256 KiB queued at once, past a
    // 100 KiB budget. Both sizes bound this fixture only.
    let part = vec![5u8; 64 * 1024];
    let (sender, mut receiver) = Socket::pair().unwrap();
    for _ in 0..4 {
        send_bytes(sender.as_raw_fd(), &frame_tagged(9, PART, &part), None).unwrap();
    }
    send_bytes(sender.as_raw_fd(), &frame_tagged(9, STATE, b"end"), None).unwrap();
    assert!(receiver.try_recv_within(100 * 1024).unwrap().is_none());
    let queued = 4 * (16 + part.len()) + 16 + 3;
    let taken = receiver.incoming.len()
        + receiver
            .partial
            .as_ref()
            .map_or(0, |partial| partial.body.len());
    assert!(
        taken < queued,
        "the first poll stopped at its budget: {taken} of {queued}"
    );
    let Some((9, ControlMessage::State(bytes))) = receiver.try_recv_within(1024 * 1024).unwrap()
    else {
        panic!("the next poll continues the message");
    };
    assert_eq!(bytes.len(), 4 * part.len() + 3);
}
