//! Document failures retain causes through the product worker and recovery.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use echode_history::{DocumentHost, recover, write_export_settings, write_package};
use echode_model::{Id, Project};
use serde_json::json;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-document-errors-{}", Id::generate()));
        write_package(&root, &Project::new()).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cause<T: Error + 'static>(error: &dyn Error) -> Option<&T> {
    let mut source = error.source();
    while let Some(error) = source {
        if let Some(error) = error.downcast_ref::<T>() {
            return Some(error);
        }
        source = error.source();
    }
    None
}

#[test]
fn transaction_import_retains_decoder_cause_and_rolls_back() {
    let fixture = Fixture::new();
    let host = DocumentHost::open(&fixture.0).unwrap();
    let before = host.snapshot();
    let error = host.apply(vec![
        json!({"type": "track.add", "name": "must roll back"}),
        json!({"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": fixture.0.join("missing.wav")}),
    ]).unwrap_err();
    assert!(
        cause::<echode_history::MediaError>(&error).is_some(),
        "transaction lost its decoder cause: {error:?}"
    );
    assert!(matches!(
        cause::<echode_history::MediaError>(&error),
        Some(echode_history::MediaError::Open(_))
    ));
    assert_eq!(*host.snapshot(), *before);
}

#[test]
fn journal_text_failure_retains_utf8_cause_and_original_bytes() {
    let fixture = Fixture::new();
    let bytes = [0xff, b'\n'];
    fs::write(fixture.0.join("journal.jsonl"), bytes).unwrap();
    let error = recover(&fixture.0).unwrap_err();
    assert!(
        cause::<std::str::Utf8Error>(&error).is_some(),
        "journal lost its UTF-8 cause: {error:?}"
    );
    assert_eq!(fs::read(fixture.0.join("journal.jsonl")).unwrap(), bytes);
}

#[test]
fn export_settings_failure_retains_json_cause_and_original_preferences() {
    let fixture = Fixture::new();
    write_export_settings(&fixture.0, "{\"version\":1}").unwrap();
    let error = write_export_settings(&fixture.0, "{\n").unwrap_err();
    assert!(
        cause::<serde_json::Error>(&error).is_some(),
        "export settings lost their JSON cause: {error:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("export.json")).unwrap(),
        "{\"version\":1}"
    );
}

#[test]
fn transaction_references_and_parameter_failures_keep_kind_index_and_text() {
    use echode_history::{ElementError, HostError, ReferenceError};

    let fixture = Fixture::new();
    let host = DocumentHost::open(&fixture.0).unwrap();
    let before = host.snapshot();
    let cases = [
        (json!({"ref": "0", "field": "id"}), ReferenceError::NotIndex),
        (
            json!({"ref": 0, "field": null}),
            ReferenceError::MissingField,
        ),
        (
            json!({"ref": u64::MAX, "field": "id"}),
            ReferenceError::OutOfRange,
        ),
        (
            json!({"ref": 0, "field": "absent"}),
            ReferenceError::FieldNotFound,
        ),
    ];
    for (reference, expected) in cases {
        let error = host
            .apply(vec![
                json!({"type": "track.add", "name": "rolled back"}),
                json!({"type": "track.add_child", "parent": reference, "name": "new"}),
            ])
            .unwrap_err();
        assert!(matches!(
            &error,
            HostError::Element { index: 1, error: ElementError::Reference(actual) }
                if *actual == expected
        ));
        assert_eq!(cause::<ReferenceError>(&error), Some(&expected));
        assert_eq!(error.to_string(), expected.to_string());
        assert_eq!(*host.snapshot(), *before);
    }
    let error = host
        .apply(vec![json!({"type": "not.a.command"})])
        .unwrap_err();
    let HostError::Element {
        index: 0,
        error: ElementError::Parameters(json),
    } = &error
    else {
        panic!("{error:?}")
    };
    assert!(cause::<serde_json::Error>(&error).is_some());
    assert_eq!(error.to_string(), echode_actions::parameter_message(json));
    assert_eq!(*host.snapshot(), *before);

    let error = host
        .apply(vec![
            json!({"type": "track.add_child", "parent": Id::generate(), "name": "missing"}),
        ])
        .unwrap_err();
    assert!(matches!(
        &error,
        HostError::Element {
            index: 0,
            error: ElementError::Action(_)
        }
    ));
    assert!(cause::<echode_model::ModelError>(&error).is_some());
    assert_eq!(host.history_usage().unwrap().undo_steps, 0);
}

#[test]
fn journal_decode_refusal_keeps_action_cause_without_repairing_a_complete_line() {
    let fixture = Fixture::new();
    let original = b"not json\n";
    fs::write(fixture.0.join("journal.jsonl"), original).unwrap();
    let error = recover(&fixture.0).unwrap_err();
    assert!(matches!(
        &error,
        echode_history::PackageError::Journal(echode_history::JournalError::Decode(_))
    ));
    assert_eq!(
        cause::<echode_actions::ActionError>(&error),
        Some(&echode_actions::ActionError::CorruptJournal)
    );
    assert_eq!(error.to_string(), "journal entry is not valid");
    assert_eq!(fs::read(fixture.0.join("journal.jsonl")).unwrap(), original);
}

#[test]
fn settings_refusals_preserve_the_boundary_and_previous_files() {
    use echode_history::{
        PackageError, SettingsError, TransportSettings, write_transport_settings,
    };

    let fixture = Fixture::new();
    // This limit applies only to preferences, not to a project or its tracks.
    let mut bounded = "{}".to_string();
    bounded.extend(std::iter::repeat_n(' ', 16_384 - bounded.len()));
    write_export_settings(&fixture.0, &bounded).unwrap();
    let oversized = format!("{bounded} ");
    let error = write_export_settings(&fixture.0, &oversized).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Settings(SettingsError::ExportTooLarge)
    ));
    assert_eq!(error.to_string(), "export settings exceed 16 KiB");
    assert_eq!(
        fs::read_to_string(fixture.0.join("export.json")).unwrap(),
        bounded
    );
    write_transport_settings(&fixture.0, TransportSettings { start: 42 }).unwrap();
    let before = fs::read(fixture.0.join("transport.json")).unwrap();
    let error = write_transport_settings(&fixture.0, TransportSettings { start: -1 }).unwrap_err();
    assert!(matches!(
        error,
        PackageError::Settings(SettingsError::NegativePlaybackStart)
    ));
    assert_eq!(error.to_string(), "playback start must not be negative");
    assert_eq!(fs::read(fixture.0.join("transport.json")).unwrap(), before);
}

#[test]
fn document_open_keeps_io_and_already_open_refusals_distinct() {
    use echode_history::{DocumentError, PackageError};

    let fixture = Fixture::new();
    let error = DocumentHost::open(&fixture.0.join("absent")).err().unwrap();
    assert_eq!(
        cause::<std::io::Error>(&error).unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
    let host = DocumentHost::open(&fixture.0).unwrap();
    let error = DocumentHost::open(&fixture.0).err().unwrap();
    assert!(matches!(
        error,
        PackageError::Document(DocumentError::AlreadyOpen)
    ));
    assert_eq!(error.to_string(), "package is already open");
    let error = host
        .perform(echode_actions::Command::Restore {
            revision: "not a revision".into(),
        })
        .unwrap_err();
    assert!(matches!(
        &error,
        echode_history::HostError::Package(PackageError::Document(DocumentError::InvalidRevision))
    ));
    assert_eq!(error.to_string(), "saved revision was not found");
    assert_eq!(host.history_usage().unwrap().undo_steps, 0);
}

#[test]
fn recovery_consent_retains_the_exact_notice_until_checkpointed() {
    use echode_actions::{Command, JournalLine, JournalOp, Session};
    use echode_history::{DocumentError, HostError, PackageError};

    let fixture = Fixture::new();
    let mut session = Session::open(Project::new());
    session
        .perform(Command::AddTrack {
            name: "past gap".into(),
        })
        .unwrap();
    let entry = session.journal().last().unwrap();
    let line = JournalLine {
        seq: 2,
        op: JournalOp::Edit,
        actions: entry.actions.clone(),
        changes: entry.changes.clone().unwrap(),
    };
    echode_history::append_journal(&fixture.0, &line).unwrap();
    let host = DocumentHost::open(&fixture.0).unwrap();
    let notice = host.recovery_notice().unwrap().clone();
    assert_eq!(notice.reason, echode_history::RecoveryReason::JournalGap);
    let error = host.require_recovery_confirmation().unwrap_err();
    assert!(
        matches!(&error, PackageError::Document(DocumentError::RecoveryConfirmationRequired(actual)) if actual == &notice)
    );
    assert_eq!(
        error.to_string(),
        format!(
            "recovery confirmation required: journal lines after a missing one were not replayed; recovered base {}; applied journal seq {}. Preserve a copy before continuing",
            notice.base, notice.applied_seq
        )
    );
    let error = host.accept_recovery("wrong tip", &notice.base).unwrap_err();
    assert!(matches!(
        error,
        HostError::Package(PackageError::Document(DocumentError::RecoveryMismatch))
    ));
    assert!(host.recovery_notice().is_some());
    host.accept_recovery(&notice.tip, &notice.base).unwrap();
    assert!(host.recovery_notice().is_none());
    host.perform(Command::AddTrack {
        name: "accepted".into(),
    })
    .unwrap();
    assert_eq!(host.snapshot().tracks().len(), 1);
}
