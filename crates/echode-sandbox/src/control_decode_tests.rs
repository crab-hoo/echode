use super::*;

fn receive(typ: u32, body: &[u8]) -> io::Result<(u64, ControlMessage)> {
    let (sender, mut receiver) = Socket::pair()?;
    send_bytes(sender.as_raw_fd(), &frame_tagged(73, typ, body)?, None)?;
    receiver.recv_tagged(Duration::from_secs(2))
}

fn assert_cause(error: io::Error, expected: ControlDecodeError, diagnostic: &str) {
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert_eq!(error.to_string(), diagnostic);
    let cause = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<ControlDecodeError>())
        .expect("malformed control metadata must retain its typed structural cause");
    assert_eq!(cause, &expected);
    assert!(std::error::Error::source(cause).is_none());
}

#[test]
fn malformed_control_metadata_retains_a_structural_cause() {
    for (typ, field, expected, diagnostic) in [
        (
            PLUGIN_RESTARTED,
            ControlField::RestartLatency,
            4,
            "restart notice is the wrong size",
        ),
        (
            SET_CHAIN,
            ControlField::ChainStages,
            4,
            "chain message is the wrong size",
        ),
        (
            REMAP,
            ControlField::RemapLayout,
            20,
            "remap layout is the wrong size",
        ),
        (
            GUI_COMMAND,
            ControlField::PluginCommand,
            1,
            "unknown control message",
        ),
    ] {
        // These byte lengths bound only the malformed fixture, not product metadata.
        for actual in (0..=24).filter(|actual| *actual != expected) {
            assert_cause(
                receive(typ, &vec![0; actual]).unwrap_err(),
                ControlDecodeError::WrongSize {
                    field,
                    expected,
                    actual,
                },
                diagnostic,
            );
        }
    }
    for typ in [PARAMS_REQUEST, GUI_CLOSE, GUI_CLOSED_REPLY, GUI_CLOSED] {
        for actual in 1..=4 {
            assert_cause(
                receive(typ, &vec![0; actual]).unwrap_err(),
                ControlDecodeError::WrongSize {
                    field: ControlField::EmptyBody { message_type: typ },
                    expected: 0,
                    actual,
                },
                "unknown control message",
            );
        }
    }
    for typ in [0, 35, 256, u32::MAX] {
        assert_cause(
            receive(typ, &[1, 2, 3]).unwrap_err(),
            ControlDecodeError::UnknownMessageType(typ),
            "unknown control message",
        );
    }
    for code in 14..=u8::MAX {
        assert_cause(
            receive(GUI_COMMAND, &[code]).unwrap_err(),
            ControlDecodeError::UnknownPluginCommand(code),
            "unknown plugin command",
        );
    }
    assert_cause(
        receive(REMAP, &layout_bytes(SessionLayout::stereo())).unwrap_err(),
        ControlDecodeError::MissingMapping,
        "remap did not include a mapping",
    );
}

#[test]
fn malformed_ports_retain_direction_field_and_lengths() {
    for actual in 0..8 {
        assert_cause(
            receive(PORTS, &vec![0; actual]).unwrap_err(),
            ControlDecodeError::WrongSize {
                field: ControlField::PortTailLatency,
                expected: 8,
                actual,
            },
            "port tail/latency is missing",
        );
    }
    for input in [true, false] {
        let prefix = if input {
            vec![]
        } else {
            0u32.to_le_bytes().to_vec()
        };
        for actual in 0..4 {
            let mut body = prefix.clone();
            body.extend(std::iter::repeat_n(0, actual));
            body.extend_from_slice(&[0; 8]);
            assert_cause(
                receive(PORTS, &body).unwrap_err(),
                ControlDecodeError::WrongSize {
                    field: ControlField::PortCount { input },
                    expected: 4,
                    actual,
                },
                "port description is truncated",
            );
        }
        for count in [1u32, 2, u32::MAX] {
            let expected = count as usize * 4;
            // A huge declared count must fail before allocation from that count.
            for actual in (0..=5).filter(|actual| *actual < expected) {
                let mut body = prefix.clone();
                body.extend_from_slice(&count.to_le_bytes());
                body.extend(std::iter::repeat_n(0, actual));
                body.extend_from_slice(&[0; 8]);
                assert_cause(
                    receive(PORTS, &body).unwrap_err(),
                    ControlDecodeError::WrongSize {
                        field: ControlField::PortChannels { input },
                        expected,
                        actual,
                    },
                    "port description is truncated",
                );
            }
        }
    }
    for actual in 1..=4 {
        let mut body = port_bytes(&[], &[]);
        body.extend(std::iter::repeat_n(0, actual));
        body.extend_from_slice(&[0; 8]);
        assert_cause(
            receive(PORTS, &body).unwrap_err(),
            ControlDecodeError::TrailingPortBytes { actual },
            "port description has trailing bytes",
        );
    }
}

#[test]
fn frame_length_rejection_retains_the_declared_length_and_bounds() {
    for actual in (0..12).chain([MAX_MESSAGE + 1, u32::MAX as usize]) {
        let (sender, mut receiver) = Socket::pair().unwrap();
        send_bytes(sender.as_raw_fd(), &(actual as u32).to_le_bytes(), None).unwrap();
        assert_cause(
            receiver.recv_tagged(Duration::from_secs(2)).unwrap_err(),
            ControlDecodeError::MessageLength {
                actual,
                minimum: 12,
                maximum: MAX_MESSAGE,
            },
            "control message length is not usable",
        );
    }
    for declared in [12, MAX_MESSAGE] {
        let (sender, mut receiver) = Socket::pair().unwrap();
        send_bytes(sender.as_raw_fd(), &(declared as u32).to_le_bytes(), None).unwrap();
        assert!(receiver.try_recv_tagged().unwrap().is_none());
    }
}

#[test]
fn metadata_errors_wait_for_the_complete_frame() {
    let frame = frame_tagged(73, SET_CHAIN, &[0]).unwrap();
    for split in 0..frame.len() {
        let (sender, mut receiver) = Socket::pair().unwrap();
        if split > 0 {
            send_bytes(sender.as_raw_fd(), &frame[..split], None).unwrap();
        }
        assert!(
            receiver.try_recv_tagged().unwrap().is_none(),
            "split {split}"
        );
        send_bytes(sender.as_raw_fd(), &frame[split..], None).unwrap();
        assert_cause(
            receiver.recv_tagged(Duration::from_secs(2)).unwrap_err(),
            ControlDecodeError::WrongSize {
                field: ControlField::ChainStages,
                expected: 4,
                actual: 1,
            },
            "chain message is the wrong size",
        );
    }
}

#[test]
fn valid_metadata_keeps_its_values_and_existing_body_acceptance() {
    let mut messages = vec![
        ControlMessage::ParamsRequest,
        ControlMessage::GuiClose,
        ControlMessage::GuiClosedReply,
        ControlMessage::GuiClosed,
    ];
    for value in [0, 1, u32::MAX] {
        messages.push(ControlMessage::SetChain(value));
        messages.push(ControlMessage::PluginRestarted {
            latency_samples: value,
        });
    }
    for code in 0..=13 {
        messages.push(ControlMessage::GuiCommand(
            crate::PluginCommand::decode(code).unwrap(),
        ));
    }
    for (inputs, outputs) in [(vec![], vec![]), (vec![0, 2], vec![1, u32::MAX])] {
        messages.push(ControlMessage::Ports {
            inputs,
            outputs,
            tail_samples: u32::MAX,
            latency_samples: 123,
        });
    }
    for message in messages {
        let (typ, body) = encode(&message);
        let (id, actual) = receive(typ, &body).unwrap();
        assert_eq!(id, 73);
        assert_eq!(format!("{actual:?}"), format!("{message:?}"));
        assert_eq!(encode(&actual), (typ, body));
    }
    // These older messages deliberately ignore bodies. Do not silently tighten
    // the accepted protocol while changing how malformed metadata is reported.
    for typ in [
        HELLO,
        SAVE,
        RESTORED,
        REQUEST_PROCESS,
        DIRTY,
        REMAPPED,
        REMAP_FAILED,
        CANCEL_REMAP,
        REMAP_CANCELLED,
        CHAIN_READY,
        CHAIN_REJECTED,
    ] {
        let (_, expected) = receive(typ, &[]).unwrap();
        let (_, actual) = receive(typ, &[1, 2, 3]).unwrap();
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }
}
