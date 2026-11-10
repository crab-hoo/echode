use super::*;
use std::ffi::CString;
use std::io;

fn check(error: HostError, expected: LaunchFailureKind) {
    let diagnostic = error.to_string();
    let Refusal::Launch(failure) = launch_refusal(&error) else {
        panic!("launch refusal lost its kind: {error:?}");
    };
    assert_eq!(failure.kind, expected);
    assert_eq!(failure.diagnostic, diagnostic);
    assert_eq!(failure.to_string(), diagnostic);
}

#[test]
fn entry_causes_keep_native_metadata_and_the_original_diagnostic() {
    use LaunchFailureKind as Kind;
    for (error, expected) in [
        (PluginEntryError::ResolveFailed, Kind::ResolveFailed),
        (PluginEntryError::NullEntryPointer, Kind::NullEntryPointer),
        (PluginEntryError::EntryInitFailed, Kind::EntryInitFailed),
        (
            PluginEntryError::Io(io::Error::from_raw_os_error(libc::EACCES)),
            Kind::EntryIo {
                raw_os_error: Some(libc::EACCES),
            },
        ),
        (
            PluginEntryError::Io(io::Error::other("custom I/O")),
            Kind::EntryIo { raw_os_error: None },
        ),
        (
            PluginEntryError::InvalidNulPath(CString::new(b"bad\0path".to_vec()).unwrap_err()),
            Kind::InvalidNulPath { position: 3 },
        ),
        (
            PluginEntryError::IncompatibleClapVersion {
                plugin_version: ClapVersion {
                    major: 42,
                    minor: 7,
                    revision: 3,
                },
            },
            Kind::IncompatibleClapVersion {
                plugin: [42, 7, 3],
                host: [
                    ClapVersion::CURRENT.major,
                    ClapVersion::CURRENT.minor,
                    ClapVersion::CURRENT.revision,
                ],
            },
        ),
    ] {
        check(HostError::Entry(error), expected);
    }
    check(HostError::NoBinary("fixture.clap".into()), Kind::NoBinary);
    check(HostError::NoFactory("fixture.clap".into()), Kind::NoFactory);
    check(HostError::NoPlugin("fixture.clap".into()), Kind::NoPlugin);
    check(HostError::NoPluginId, Kind::NoPluginId);
}

#[test]
fn every_instance_cause_is_classified_without_duplicating_its_words() {
    use LaunchFailureKind as Kind;
    use PluginInstanceError as Error;
    for (error, expected) in [
        (Error::StartProcessingFailed, Kind::StartProcessingFailed),
        (Error::AlreadyActivatedPlugin, Kind::AlreadyActivatedPlugin),
        (Error::StillActivatedPlugin, Kind::StillActivatedPlugin),
        (Error::DeactivatedPlugin, Kind::DeactivatedPlugin),
        (Error::ActivationFailed, Kind::ActivationFailed),
        (Error::PluginNotFound, Kind::PluginNotFound),
        (Error::MissingPluginFactory, Kind::MissingPluginFactory),
        (Error::InstantiationFailed, Kind::InstantiationFailed),
        (Error::PluginDestroyed, Kind::PluginDestroyed),
        (Error::ProcessingFailed, Kind::ProcessingFailed),
        (Error::ProcessingStopped, Kind::ProcessingStopped),
        (Error::ProcessingStarted, Kind::ProcessingStarted),
        (
            Error::NullFactoryCreatePluginFunction,
            Kind::NullFactoryCreatePluginFunction,
        ),
        (Error::NullProcessFunction, Kind::NullProcessFunction),
        (Error::NullActivateFunction, Kind::NullActivateFunction),
    ] {
        check(HostError::Instance(error), expected);
    }
}

#[test]
fn plugin_failures_keep_the_existing_kind() {
    for failure in [
        echode_sandbox::PluginFailure::NoAudioPorts,
        echode_sandbox::PluginFailure::CapacityRefused,
    ] {
        assert_eq!(
            launch_refusal(&HostError::Plugin(failure)),
            Refusal::Plugin(failure)
        );
    }
}
