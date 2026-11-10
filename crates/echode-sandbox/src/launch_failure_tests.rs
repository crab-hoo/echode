use super::*;
use crate::refusal::{Refusal, read_refusal, write_refusal};

fn every_kind() -> Vec<LaunchFailureKind> {
    use LaunchFailureKind::*;
    vec![
        Library,
        InvalidNulPath { position: u64::MAX },
        EntryIo { raw_os_error: None },
        EntryIo {
            raw_os_error: Some(i32::MIN),
        },
        EntryIo {
            raw_os_error: Some(i32::MAX),
        },
        ResolveFailed,
        NullEntryPointer,
        IncompatibleClapVersion {
            plugin: [u32::MAX, 0, 1],
            host: [1, 2, u32::MAX],
        },
        EntryInitFailed,
        UnknownEntry,
        NoBinary,
        NoFactory,
        NoPlugin,
        NoPluginId,
        StartProcessingFailed,
        AlreadyActivatedPlugin,
        StillActivatedPlugin,
        DeactivatedPlugin,
        ActivationFailed,
        PluginNotFound,
        MissingPluginFactory,
        InstantiationFailed,
        PluginDestroyed,
        ProcessingFailed,
        ProcessingStopped,
        ProcessingStarted,
        NullFactoryCreatePluginFunction,
        NullProcessFunction,
        NullActivateFunction,
    ]
}

#[test]
fn launch_causes_and_native_metadata_round_trip_through_refusals() {
    let mut codes = std::collections::BTreeSet::new();
    for kind in every_kind() {
        for diagnostic in ["", "plugin refused — état", "Unable to activate"] {
            let failure = LaunchFailure {
                kind,
                diagnostic: diagnostic.into(),
            };
            assert_eq!(failure.to_string(), diagnostic);
            codes.insert(failure.encode()[0]);
            let refusal = Refusal::Launch(failure);
            let bytes = write_refusal(&refusal);
            assert_eq!(read_refusal(&bytes).unwrap(), refusal);
            assert_eq!(refusal.to_string(), diagnostic);
        }
    }
    // EntryIo appears three times with different native metadata.
    assert_eq!(codes.len(), every_kind().len() - 2);
}

#[test]
fn truncated_metadata_unknown_causes_and_non_text_are_rejected() {
    for kind in every_kind() {
        let encoded = LaunchFailure {
            kind,
            diagnostic: String::new(),
        }
        .encode();
        // With an empty diagnostic, every proper prefix lacks required metadata.
        for end in 0..encoded.len() {
            assert!(
                LaunchFailure::decode(&encoded[..end]).is_err(),
                "{kind:?}: {end}"
            );
        }
        let mut invalid_text = encoded;
        invalid_text.push(0xff);
        let expected = std::str::from_utf8(&invalid_text[invalid_text.len() - 1..]).unwrap_err();
        assert_eq!(
            LaunchFailure::decode(&invalid_text).unwrap_err(),
            LaunchDecodeError::Diagnostic(expected)
        );
    }
    assert_eq!(
        LaunchFailure::decode(&[2, 2]).unwrap_err(),
        LaunchDecodeError::InvalidErrnoFlag(2)
    );
    for code in 27..=u8::MAX {
        assert_eq!(
            LaunchFailure::decode(&[code]).unwrap_err(),
            LaunchDecodeError::UnknownKind(code)
        );
    }
}

#[test]
fn every_accepted_encoding_is_canonical() {
    for code in 0..=u8::MAX {
        // Probe every cause code, including metadata flags, lengths and UTF-8.
        for byte in [0, 1, 2, 0x7f, 0xff] {
            for len in 0..=32 {
                let mut bytes = vec![code];
                bytes.extend(std::iter::repeat_n(byte, len));
                if let Ok(failure) = LaunchFailure::decode(&bytes) {
                    assert_eq!(failure.encode(), bytes);
                }
            }
        }
    }
}
