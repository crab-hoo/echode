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

/// The helper is untrusted. A body cut short anywhere, a remap layout of the
/// wrong size, or a payload without a type is refused as invalid data, not
/// a panic (ADR 0284).
#[test]
fn every_truncated_body_is_refused_without_a_panic() {
    let messages = [
        ControlMessage::Ports {
            inputs: vec![2, 1],
            outputs: vec![2],
            tail_samples: 7,
            latency_samples: 64,
        },
        ControlMessage::SetChain(3),
        ControlMessage::PluginRestarted { latency_samples: 9 },
        ControlMessage::GuiCommand(crate::PluginCommand::decode(0).unwrap()),
        ControlMessage::Params(vec![ParamInfo {
            id: 4,
            flags: 0,
            name: "Cutoff".into(),
            module: "Filter".into(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            value: 0.25,
            text: "25 %".into(),
        }]),
        ControlMessage::SetParams(vec![ParamChange { id: 4, value: 0.5 }]),
        ControlMessage::ParamsApplied(vec![ParamValue {
            id: 4,
            value: 0.5,
            text: "50 %".into(),
        }]),
    ];
    for message in &messages {
        let (typ, body) = encode(message).unwrap();
        for end in 0..body.len() {
            let mut payload = typ.to_le_bytes().to_vec();
            payload.extend_from_slice(&body[..end]);
            if let Err(error) = decode(&payload, None) {
                assert_eq!(error.kind(), ErrorKind::InvalidData, "{message:?} at {end}");
            }
        }
    }
    for len in (0..24).filter(|len| *len != 20) {
        let mut payload = REMAP.to_le_bytes().to_vec();
        payload.resize(4 + len, 0);
        assert_eq!(
            decode(&payload, None).unwrap_err().kind(),
            ErrorKind::InvalidData,
            "remap layout of {len} bytes"
        );
    }
    for len in 0..4 {
        assert_eq!(
            decode(&vec![0; len], None).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }
}
