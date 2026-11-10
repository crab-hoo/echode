//! Command decoding keeps typed causes through the product document worker.

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use echode_actions::ParameterError;
use echode_document::{DocumentError, DocumentService, Operation, Presenter, Publication, Ticket};
use echode_history::{DocumentHost, write_package};
use echode_model::{Id, Project};
use serde_json::json;

struct Projects;

impl Presenter for Projects {
    type View = Arc<Project>;

    fn present(&mut self, publication: Publication<'_>) -> Self::View {
        publication.project
    }

    fn with_device_report(view: Self::View, _report: Vec<u8>) -> Self::View {
        view
    }
}

struct Fixture {
    service: DocumentService<Arc<Project>>,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-edit-errors-{}", Id::generate()));
        write_package(&root, &Project::new()).unwrap();
        Self {
            service: DocumentService::open(&root, Projects).unwrap(),
            root,
        }
    }

    fn finish(&self, operation: Operation) -> Result<Arc<Project>, DocumentError> {
        let (sender, receiver) = mpsc::channel();
        self.service.submit(
            operation,
            Ticket::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        receiver.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn edit(&self, value: serde_json::Value) -> Result<Arc<Project>, DocumentError> {
        self.finish(Operation::Perform(serde_json::to_vec(&value).unwrap()))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn cause<T: Error + 'static>(error: &dyn Error) -> Option<&T> {
    let mut source = error.source();
    while let Some(error) = source {
        if let Some(cause) = error.downcast_ref::<T>() {
            return Some(cause);
        }
        source = error.source();
    }
    None
}

#[test]
fn single_edit_value_failures_keep_their_causes_redo_and_journal() {
    let fixture = Fixture::new();
    let added = fixture
        .edit(json!({"type": "track.add", "name": "kept"}))
        .unwrap();
    let before = fixture.finish(Operation::Undo).unwrap();
    let journal = std::fs::read(fixture.root.join("journal.jsonl")).unwrap();
    let invalid_id = "broken at line 7: meter does not divide into ticks\n音";
    let cases = [
        json!({"type": "track.duplicate", "source": invalid_id}),
        json!({"type": "project.set_meter", "numerator": 1, "denominator": 27}),
        json!({"type": "project.set_tempo", "at": 0, "numerator": 120, "denominator": 0}),
    ];
    for (index, value) in cases.into_iter().enumerate() {
        let error = fixture.edit(value).unwrap_err();
        let parameters = cause::<ParameterError>(&error)
            .unwrap_or_else(|| panic!("single edit erased its parameter cause: {error:?}"));
        match (index, parameters) {
            (
                0,
                ParameterError::InvalidId {
                    field: "source",
                    value,
                },
            ) => {
                assert_eq!(value, invalid_id);
                assert_eq!(error.to_string(), format!("invalid id {invalid_id}"));
            }
            (
                1,
                ParameterError::InvalidMeter {
                    numerator: 1,
                    denominator: 27,
                },
            ) => {
                assert_eq!(error.to_string(), "meter does not divide into ticks");
            }
            (
                2,
                ParameterError::InvalidTempo {
                    numerator: 120,
                    denominator: 0,
                },
            ) => {
                assert_eq!(error.to_string(), "tempo is outside the allowed range");
            }
            _ => panic!("wrong single-edit cause: {error:?}"),
        }
        assert!(parameters.source().is_none());
        assert!(cause::<serde_json::Error>(&error).is_none());
        let unchanged = fixture.finish(Operation::Snapshot).unwrap();
        assert!(Arc::ptr_eq(&before, &unchanged));
        assert_eq!(
            std::fs::read(fixture.root.join("journal.jsonl")).unwrap(),
            journal
        );
    }
    let redone = fixture.finish(Operation::Redo).unwrap();
    assert_eq!(*redone, *added);
    fixture.finish(Operation::Undo).unwrap();
    fixture.service.shutdown();
    let reopened = DocumentHost::open(&fixture.root).unwrap();
    assert_eq!(*reopened.snapshot(), *before);
}

#[test]
fn malformed_single_edits_keep_json_diagnostics_and_never_change_history() {
    use echode_actions::Command;

    let fixture = Fixture::new();
    let added = fixture
        .edit(json!({"type": "track.add", "name": "kept"}))
        .unwrap();
    let before = fixture.finish(Operation::Undo).unwrap();
    let journal = std::fs::read(fixture.root.join("journal.jsonl")).unwrap();
    for text in [
        " \n ",
        "{\n",
        "null",
        r#"{"type":"track.add","name":7}"#,
        r#"{"type":"track.add","name":"first","name":"second"}"#,
        r#"{"type":"track.add","name":"x"} {}"#,
        r#"{"type":"project.set_meter","numerator":1,"denominator":"27"}"#,
        r#"{"type":"tempo is outside the allowed range"}"#,
        r#"{"type":"track.add","name":"x","invalid id user text":0}"#,
    ] {
        let previous = serde_json::from_str::<Command>(text).unwrap_err();
        let error = fixture
            .finish(Operation::Perform(text.as_bytes().to_vec()))
            .unwrap_err();
        let DocumentError::Malformed(json) = &error else {
            panic!("shape or syntax was misclassified: {text}: {error:?}")
        };
        assert_eq!(json.classify(), previous.classify());
        assert_eq!(
            (json.line(), json.column()),
            (previous.line(), previous.column())
        );
        assert_eq!(error.to_string(), previous.to_string());
        assert!(cause::<serde_json::Error>(&error).is_some());
        assert!(cause::<ParameterError>(&error).is_none());
        let unchanged = fixture.finish(Operation::Snapshot).unwrap();
        assert!(Arc::ptr_eq(&before, &unchanged));
        assert_eq!(
            std::fs::read(fixture.root.join("journal.jsonl")).unwrap(),
            journal
        );
    }
    for (bytes, expected) in [
        (vec![], echode_document::Rejected::EmptyCommand),
        (vec![0xff], echode_document::Rejected::NotUtf8),
    ] {
        let error = fixture.finish(Operation::Perform(bytes)).unwrap_err();
        assert!(matches!(error, DocumentError::Rejected(actual) if actual == expected));
    }
    assert_eq!(*fixture.finish(Operation::Redo).unwrap(), *added);
}

#[test]
fn single_and_batch_edits_report_the_same_semantic_cause() {
    use echode_history::{ElementError, HostError};

    let fixture = Fixture::new();
    let before = fixture.finish(Operation::Snapshot).unwrap();
    for value in [
        json!({"type": "track.duplicate", "source": "broken at line 7"}),
        json!({"type": "project.set_meter", "numerator": 1, "denominator": 27}),
        json!({"type": "project.set_tempo", "at": 0, "numerator": 0, "denominator": 1}),
        json!({"type": "track.set_parent", "track": Id::generate(), "parent": "bad parent"}),
    ] {
        let single = fixture.edit(value.clone()).unwrap_err();
        assert!(matches!(&single, DocumentError::Parameters(_)));
        let batch = fixture
            .edit(json!([
                {"type": "track.add", "name": "must roll back"},
                value,
            ]))
            .unwrap_err();
        assert!(matches!(
            &batch,
            DocumentError::History(HostError::Element {
                index: 1,
                error: ElementError::Parameters(_)
            })
        ));
        let single_cause = cause::<ParameterError>(&single).unwrap();
        let batch_cause = cause::<ParameterError>(&batch).unwrap();
        assert_eq!(format!("{single_cause:?}"), format!("{batch_cause:?}"));
        assert_eq!(single.to_string(), batch.to_string());
        let unchanged = fixture.finish(Operation::Snapshot).unwrap();
        assert!(Arc::ptr_eq(&before, &unchanged));
    }
    let added = fixture
        .edit(json!({"type": "track.add", "name": "after refusal"}))
        .unwrap();
    assert_eq!(added.tracks().len(), 1);
    assert_eq!(*fixture.finish(Operation::Undo).unwrap(), *before);
}
