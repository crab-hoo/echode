use std::error::Error;
use std::io;
use std::path::PathBuf;

use super::*;
use echode_engine::{PlaybackError, PluginBlockFailure, PluginBlockFailureReason};
use echode_history::{HostError, PackageError};

#[test]
fn forwarding_preserves_nested_causes_and_display() {
    let package = PackageError::Io {
        path: Some(PathBuf::from("missing-state")),
        error: io::Error::from_raw_os_error(2),
    };
    let expected = package.to_string();
    let error = CliError::from(HostError::Package(package));
    assert_eq!(error.to_string(), expected);
    let host = error.source().unwrap().downcast_ref::<HostError>().unwrap();
    let package = host
        .source()
        .unwrap()
        .downcast_ref::<PackageError>()
        .unwrap();
    assert!(
        matches!(package, PackageError::Io { path: Some(path), .. } if path == std::path::Path::new("missing-state"))
    );
    assert_eq!(
        package
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(2)
    );

    let error = CliError::from(echode_audio_io::AudioError::Playback(
        PlaybackError::BufferTooSmall,
    ));
    assert_eq!(error.to_string(), PlaybackError::BufferTooSmall.to_string());
    assert!(matches!(
        error
            .source()
            .unwrap()
            .downcast_ref::<echode_audio_io::AudioError>(),
        Some(echode_audio_io::AudioError::Playback(
            PlaybackError::BufferTooSmall
        ))
    ));
    let error = CliError::from(PlaybackError::NoRunningPlugin);
    assert!(matches!(
        error.source().unwrap().downcast_ref::<PlaybackError>(),
        Some(PlaybackError::NoRunningPlugin)
    ));
    assert_eq!(error.to_string(), "the device has no running plugin");
}

#[test]
fn plugin_block_keeps_device_identity_and_every_failure_reason() {
    for (reason, text) in [
        (PluginBlockFailureReason::ProcessRefused, "process refused"),
        (
            PluginBlockFailureReason::Deadline,
            "did not finish before the deadline",
        ),
        (PluginBlockFailureReason::Submit, "could not submit audio"),
        (
            PluginBlockFailureReason::NonFinite,
            "returned a NaN or an infinity",
        ),
        (
            PluginBlockFailureReason::Exited,
            "plugin process has exited",
        ),
    ] {
        let error = CliError::PluginBlock(PluginBlockFailure {
            device: 17,
            clap_id: "test.plugin".into(),
            reason,
        });
        let cause = error
            .source()
            .unwrap()
            .downcast_ref::<PluginBlockFailure>()
            .unwrap();
        assert_eq!(cause.device, 17);
        assert_eq!(cause.clap_id, "test.plugin");
        assert_eq!(cause.reason, reason);
        assert_eq!(
            error.to_string(),
            format!("device 17 (test.plugin): {text}")
        );
        assert!(!error.is_cancelled());
        assert!(!error.is_durability_unknown());
    }
}

#[test]
fn published_output_uncertainty_keeps_io_cause_and_is_never_cancellation() {
    for injected in [false, true] {
        let cause = (!injected).then(|| io::Error::from_raw_os_error(5));
        let expected = cause.as_ref().map_or_else(
            || "export durability is unknown".to_owned(),
            |cause| format!("export durability is unknown: {cause}"),
        );
        let error = CliError::DurabilityUnknown {
            path: "mix.wav".into(),
            error: cause,
        };
        assert_eq!(error.to_string(), expected);
        assert!(error.is_durability_unknown());
        assert!(!error.is_cancelled());
        if !injected {
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(5)
            );
        } else {
            assert!(error.source().is_none());
        }
    }
    assert!(CliError::Cancelled.is_cancelled());
    assert!(!CliError::Cancelled.is_durability_unknown());
    assert_eq!(CliError::Cancelled.to_string(), "export cancelled");
}

#[test]
fn arguments_keep_values_and_parse_causes_separate_from_range_rejection() {
    for (text, parse_error) in [
        ("invalid", true),
        ("NaN", false),
        ("1", false),
        ("-121", false),
    ] {
        let error = PeakTarget::parse(text).err().unwrap();
        assert_eq!(
            error.to_string(),
            "peak target must be a finite dBFS value from -120 to 0"
        );
        assert!(
            matches!(&error, CliError::Argument(ArgumentError::PeakTarget { text: value, error })
            if value == text && error.is_some() == parse_error)
        );
        let argument = error
            .source()
            .unwrap()
            .downcast_ref::<ArgumentError>()
            .unwrap();
        assert_eq!(argument.source().is_some(), parse_error);
    }
    assert!(
        matches!(WavFormat::parse("mp3"), Err(CliError::Argument(ArgumentError::WavFormat(value))) if value == "mp3")
    );
    assert!(
        matches!(Dither::parse("unknown"), Err(CliError::Argument(ArgumentError::Dither(value))) if value == "unknown")
    );
    assert!(matches!(
        Dither::Tpdf.validate(WavFormat::Float32),
        Err(CliError::Argument(ArgumentError::DitherFormat))
    ));
    assert!(
        std::mem::size_of::<CliError>() <= 128,
        "large boundary causes should be boxed"
    );
}
