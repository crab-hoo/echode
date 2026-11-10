use super::*;
use std::error::Error;

#[test]
fn durability_is_a_variant_not_a_message_match() {
    let unknown = PackageError::durability_unknown();
    assert!(matches!(unknown, PackageError::JournalDurabilityUnknown));
    assert!(unknown.is_durability_unknown());
    let text = PackageError::from(std::io::Error::other(unknown.to_string()));
    assert!(!text.is_durability_unknown());
    assert_eq!(text.to_string(), unknown.to_string());
}

#[test]
fn storage_errors_keep_os_codes_and_allocation_causes() {
    let error = PackageError::Io {
        path: Some(PathBuf::from("Song.echode/project.json")),
        error: std::io::Error::from_raw_os_error(libc::EACCES),
    };
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::EACCES)
    );
    assert!(error.to_string().starts_with("Song.echode/project.json: "));
    let cause = Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err();
    let error = PackageError::Audio(MediaError::Allocation(cause));
    assert_eq!(error.to_string(), "audio is too large to decode");
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<TryReserveError>()
    );
}

#[test]
fn native_decoder_details_do_not_change_the_display_text() {
    let property = MediaError::Property {
        property: 42,
        status: 0,
        expected_size: 8,
        actual_size: 4,
    };
    assert_eq!(property.to_string(), "could not read an audio property (0)");
    assert!(matches!(
        property,
        MediaError::Property {
            property: 42,
            status: 0,
            expected_size: 8,
            actual_size: 4,
        }
    ));
    let setting = MediaError::SetProperty {
        property: 43,
        status: -50,
    };
    assert_eq!(
        setting.to_string(),
        "could not set the audio client format (-50)"
    );
    assert!(matches!(
        setting,
        MediaError::SetProperty {
            property: 43,
            status: -50
        }
    ));
    assert_eq!(
        MediaError::UnsupportedChannels(6).to_string(),
        "only mono and stereo audio can be imported"
    );
    assert_eq!(
        MediaError::InvalidSampleRate(1.5).to_string(),
        "audio sample rate is not a positive integer"
    );
    assert_eq!(
        MediaError::SampleRateTooHigh(f64::MAX).to_string(),
        "audio sample rate is too high"
    );
}

#[test]
fn autosave_timeouts_are_not_worker_shutdowns() {
    use std::sync::mpsc::RecvTimeoutError;
    assert_eq!(
        AutosaveError::from(RecvTimeoutError::Timeout),
        AutosaveError::TimedOut
    );
    assert_eq!(
        AutosaveError::from(RecvTimeoutError::Disconnected),
        AutosaveError::Stopped
    );
    assert_eq!(
        AutosaveError::TimedOut.to_string(),
        "autosave request timed out"
    );
    assert_eq!(
        AutosaveError::Stopped.to_string(),
        "autosave thread stopped"
    );
}

#[test]
fn shared_delivery_keeps_native_codes_and_durability_without_nesting() {
    let original =
        PackageError::from(std::io::Error::from_raw_os_error(libc::EACCES)).into_shared();
    let shared = PackageError::Shared(Arc::clone(&original)).into_shared();
    assert!(Arc::ptr_eq(&original, &shared));
    let delivered = PackageError::Shared(shared);
    let PackageError::Io { error, .. } = delivered.original() else {
        panic!("shared I/O cause was lost: {delivered:?}");
    };
    assert_eq!(error.raw_os_error(), Some(libc::EACCES));
    let unknown = PackageError::Shared(PackageError::durability_unknown().into_shared());
    assert!(unknown.is_durability_unknown());
    let text = PackageError::Shared(
        PackageError::from(std::io::Error::other(unknown.to_string())).into_shared(),
    );
    assert!(!text.is_durability_unknown());
}
