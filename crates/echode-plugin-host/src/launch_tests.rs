use super::*;
use std::ffi::CString;
use std::io;

fn check(error: LoadError, expected: LaunchFailureKind) {
    let diagnostic = error.to_string();
    let failure = launch_failure(&error);
    assert_eq!(failure.kind, expected);
    assert_eq!(failure.diagnostic, diagnostic);
    assert_eq!(failure.to_string(), diagnostic);
    let host_error = crate::HostError::from(error);
    assert_eq!(host_error.to_string(), diagnostic);
    let source = std::error::Error::source(&host_error)
        .unwrap()
        .downcast_ref::<LoadError>()
        .unwrap();
    assert_eq!(launch_failure(source), failure);
    match source {
        LoadError::Entry(error) => {
            let cause = std::error::Error::source(source)
                .unwrap()
                .downcast_ref::<PluginEntryError>()
                .unwrap();
            assert_eq!(cause.to_string(), error.to_string());
        }
        LoadError::Instance(error) => {
            let cause = std::error::Error::source(source)
                .unwrap()
                .downcast_ref::<PluginInstanceError>()
                .unwrap();
            assert_eq!(cause.to_string(), error.to_string());
        }
        _ => assert!(std::error::Error::source(source).is_none()),
    }
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
        check(LoadError::Entry(error), expected);
    }
    check(LoadError::NoBinary("fixture.clap".into()), Kind::NoBinary);
    check(LoadError::NoFactory("fixture.clap".into()), Kind::NoFactory);
    check(LoadError::NoPlugin("fixture.clap".into()), Kind::NoPlugin);
    check(LoadError::NoPluginId, Kind::NoPluginId);
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
        check(LoadError::Instance(error), expected);
    }
}
