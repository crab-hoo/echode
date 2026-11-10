use super::*;

fn every_kind() -> [Refusal; 7] {
    [
        Refusal::Busy,
        Refusal::NoPlugin,
        Refusal::TimedOut(Pending::MainCall(u64::MAX)),
        Refusal::TimedOut(Pending::ParameterChanges),
        Refusal::Stopped(Pending::MainCall(7)),
        Refusal::Stopped(Pending::ParameterChanges),
        Refusal::Other("plugin refused — état".to_string()),
    ]
}

const INPUT: AudioPort = AudioPort {
    input: true,
    index: 0,
};
const OUTPUT: AudioPort = AudioPort {
    input: false,
    index: u32::MAX,
};

fn every_failure() -> [PluginFailure; 27] {
    use PluginFailure::*;
    [
        NoState,
        NoStateSave,
        NoStateLoad,
        SaveRefused,
        LoadRefused,
        CapacityRefused,
        NoParameterFlush,
        NoWindow,
        NoCocoaWindow,
        NoWindowCreate,
        WindowRefused,
        NoEmbedding,
        ParentRefused,
        PanelNeedsMainThread,
        PanelHasNoView,
        NoAudioPorts,
        NoPortCount,
        NoPortInfo,
        PortUndescribed(INPUT),
        PortUndescribed(OUTPUT),
        PortWithoutChannels(INPUT),
        PortWithoutChannels(OUTPUT),
        NoPorts,
        NoNotePortCount,
        NoNotePortInfo,
        NotePortUndescribed,
        NotePortIndex,
    ]
}

#[test]
fn every_refusal_round_trips_with_its_kind() {
    let failures = every_failure().map(Refusal::Plugin);
    for refusal in every_kind().into_iter().chain(failures) {
        let bytes = write_refusal(&refusal);
        assert_eq!(read_refusal(&bytes).unwrap(), refusal);
    }
    // An empty reason is still a reason, not a missing kind.
    let empty = Refusal::Other(String::new());
    assert_eq!(read_refusal(&write_refusal(&empty)).unwrap(), empty);
}

#[test]
fn busy_is_not_text_that_happens_to_say_busy() {
    let text = Refusal::Other(Refusal::Busy.to_string());
    assert_eq!(text.to_string(), Refusal::Busy.to_string());
    assert_ne!(read_refusal(&write_refusal(&text)).unwrap(), Refusal::Busy);
}

#[test]
fn every_failure_has_its_own_code_and_keeps_the_helpers_words() {
    let mut codes = std::collections::BTreeSet::new();
    for failure in every_failure() {
        codes.insert(failure_code(failure));
    }
    // Two port kinds appear twice in the list, once for each side.
    assert_eq!(codes.len(), every_failure().len() - 2);
    // Every code the reader accepts writes back to the same bytes.
    for code in 0..=u8::MAX {
        let simple = [PLUGIN, code];
        let port = [PLUGIN, code, 1, 4, 0, 0, 0];
        for bytes in [&simple[..], &port[..]] {
            if let Ok(refusal) = read_refusal(bytes) {
                assert_eq!(write_refusal(&refusal), bytes);
            }
        }
    }
    let port = PluginFailure::PortWithoutChannels(INPUT);
    assert_eq!(port.to_string(), "input port 0 has no channels");
    let port = PluginFailure::PortUndescribed(OUTPUT);
    assert_eq!(
        port.to_string(),
        format!("plugin did not describe output port {}", u32::MAX)
    );
    assert_eq!(
        Refusal::Plugin(PluginFailure::NoState).to_string(),
        "plugin does not declare state"
    );
}

#[test]
fn malformed_refusals_are_invalid_data() {
    let mut extra = write_refusal(&Refusal::Busy);
    extra.push(0);
    let mut short = write_refusal(&Refusal::TimedOut(Pending::MainCall(3)));
    short.pop();
    for bytes in [
        Vec::new(),
        extra,
        short,
        vec![STOPPED_PARAMETERS, 1],
        vec![OTHER, 0xff],
        vec![UNEXPECTED_MAIN_REPLY + 1],
        vec![LAUNCH],
        vec![LAUNCH, 27],
        vec![LAUNCH, 0, 0xff],
        vec![LAUNCH, 2, 2],
        // A plugin failure needs its code; a port kind needs its port.
        vec![PLUGIN],
        vec![PLUGIN, 18],
        vec![PLUGIN, 18, 1, 0, 0, 0],
        vec![PLUGIN, 18, 2, 0, 0, 0, 0],
        vec![PLUGIN, 0, 1, 0, 0, 0, 0],
        vec![PLUGIN, 0, 0],
        vec![PLUGIN, u8::MAX],
        vec![INVALID_PARAMETER_ID],
        vec![INVALID_PARAMETER_ID, 0, 0, 0],
        vec![INVALID_PARAMETER_ID, 0, 0, 0, 0, 0],
        vec![UNEXPECTED_MAIN_REPLY],
        vec![UNEXPECTED_MAIN_REPLY, 0],
        vec![UNEXPECTED_MAIN_REPLY, 0, 0, 0],
        vec![UNEXPECTED_MAIN_REPLY, 6, 0],
        vec![UNEXPECTED_MAIN_REPLY, 0, 6],
    ] {
        let error = read_refusal(&bytes).expect_err("a malformed refusal was accepted");
        assert_eq!(error.kind(), ErrorKind::InvalidData, "{bytes:?}");
    }
}

#[test]
fn internal_helper_failures_keep_kinds_metadata_and_diagnostics() {
    for id in [0, u32::MAX - 1, u32::MAX] {
        let refusal = Refusal::InvalidParameterId(id);
        assert_eq!(read_refusal(&write_refusal(&refusal)).unwrap(), refusal);
        assert_eq!(refusal.to_string(), "invalid parameter change");
    }
    use MainReplyKind::*;
    let kinds = [State, Done, Parameters, Values, Parsed, Window];
    for (expected, diagnostic) in kinds.into_iter().zip([
        "unexpected main-thread reply",
        "unexpected main-thread reply",
        "unexpected main-thread reply to parameter list",
        "unexpected main-thread reply to parameter values",
        "unexpected main-thread reply to parameter text",
        "unexpected main-thread reply",
    ]) {
        for actual in kinds {
            let refusal = Refusal::UnexpectedMainReply { expected, actual };
            let bytes = write_refusal(&refusal);
            assert_eq!(read_refusal(&bytes).unwrap(), refusal);
            assert_eq!(refusal.to_string(), diagnostic);
            let text = Refusal::Other(diagnostic.to_string());
            assert_ne!(read_refusal(&write_refusal(&text)).unwrap(), refusal);
        }
    }
    // Every accepted code is canonical; neither extra data nor unknown kinds pass.
    for expected in 0..=u8::MAX {
        for actual in 0..=u8::MAX {
            let bytes = [UNEXPECTED_MAIN_REPLY, expected, actual];
            match read_refusal(&bytes) {
                Ok(refusal) => assert_eq!(write_refusal(&refusal), bytes),
                Err(error) => assert_eq!(error.kind(), ErrorKind::InvalidData),
            }
        }
    }
}
