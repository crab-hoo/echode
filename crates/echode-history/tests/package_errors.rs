//! Package validation preserves typed failures and leaves rejected input intact.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use echode_history::{read_package, write_package};
use echode_model::{Id, ModelError, NoteDraft, Project, Tick};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-package-errors-{}", Id::generate()));
        let (project, track) = Project::new().add_track("validation");
        let (project, _) = project
            .insert_note(
                track,
                NoteDraft {
                    pitch: 60,
                    velocity: 100,
                    start: Tick::new(0),
                    length: Tick::new(100),
                },
            )
            .unwrap();
        write_package(&root, &project).unwrap();
        Self(root)
    }

    fn reject(&self, text: &str) -> echode_history::PackageError {
        self.reject_file(&self.0.join("project.json"), text)
    }

    fn reject_file(&self, path: &std::path::Path, text: &str) -> echode_history::PackageError {
        let files = self.files();
        let before: Vec<_> = files
            .iter()
            .map(|(_, file)| fs::read(file).unwrap())
            .collect();
        fs::write(path, text).unwrap();
        let error = read_package(&self.0).unwrap_err();
        for ((_, file), original) in files.iter().zip(before) {
            assert_eq!(
                fs::read(file).unwrap(),
                if file == path {
                    text.as_bytes()
                } else {
                    &original
                }
            );
        }
        error
    }

    fn files(&self) -> [(echode_history::PackageKind, PathBuf); 3] {
        use echode_history::PackageKind;
        let header: serde_json::Value =
            serde_json::from_slice(&fs::read(self.0.join("project.json")).unwrap()).unwrap();
        [
            (PackageKind::Project, self.0.join("project.json")),
            (
                PackageKind::Track,
                self.0
                    .join("tracks")
                    .join(format!("{}.json", header["tracks"][0].as_str().unwrap())),
            ),
            (
                PackageKind::Content,
                self.0
                    .join("contents")
                    .join(format!("{}.json", header["contents"][0].as_str().unwrap())),
            ),
        ]
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
fn package_json_failure_retains_its_original_cause() {
    let fixture = Fixture::new();
    let error = fixture.reject("{\n");
    let json = cause::<serde_json::Error>(&error);
    assert!(
        json.is_some(),
        "package validation lost its JSON cause: {error:?}"
    );
    let json = json.unwrap();
    assert!(json.is_eof());
    assert_eq!(json.line(), 2);
    assert_eq!(error.to_string(), format!("project.json: {json}"));
}

#[test]
fn package_model_failure_retains_its_original_cause() {
    let fixture = Fixture::new();
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture.0.join("project.json")).unwrap()).unwrap();
    let track = value["tracks"][0].clone();
    value["tracks"].as_array_mut().unwrap().push(track);
    let error = fixture.reject(&value.to_string());
    assert!(
        cause::<ModelError>(&error).is_some(),
        "package validation lost its model cause: {error:?}"
    );
    assert_eq!(cause::<ModelError>(&error), Some(&ModelError::DuplicateId));
    assert_eq!(error.to_string(), ModelError::DuplicateId.to_string());
}

#[test]
fn schema_refusals_keep_kind_number_and_legacy_text() {
    use echode_history::{PackageError, PackageValidationError as Invalid};
    use serde_json::json;

    let fixture = Fixture::new();
    for (kind, path) in fixture.files() {
        let original = fs::read_to_string(&path).unwrap();
        let cases = [
            (
                json!([]),
                Invalid::NotObject(kind),
                format!("{kind} json is not an object"),
            ),
            (
                json!({}),
                Invalid::MissingSchema(kind),
                format!("{kind} schema is missing"),
            ),
            (
                json!({"schema": null}),
                Invalid::NonIntegerSchema(kind),
                format!("{kind} schema is not an integer"),
            ),
            (
                json!({"schema": -1}),
                Invalid::NonIntegerSchema(kind),
                format!("{kind} schema is not an integer"),
            ),
            (
                json!({"schema": 1.0}),
                Invalid::NonIntegerSchema(kind),
                format!("{kind} schema is not an integer"),
            ),
            (
                json!({"schema": "6"}),
                Invalid::NonIntegerSchema(kind),
                format!("{kind} schema is not an integer"),
            ),
        ];
        for (value, expected, text) in cases {
            let error = fixture.reject_file(&path, &value.to_string());
            let PackageError::Validation(actual) = &error else {
                panic!("{error:?}")
            };
            assert_eq!(
                std::mem::discriminant(actual),
                std::mem::discriminant(&expected)
            );
            assert_eq!(error.to_string(), text);
            fs::write(&path, &original).unwrap();
        }
        for schema in [0, 7, u64::from(u32::MAX) + 1, u64::MAX] {
            let error = fixture.reject_file(&path, &json!({"schema": schema}).to_string());
            assert!(
                matches!(error, PackageError::Validation(Invalid::UnsupportedSchema {
                kind: found, schema: number,
            }) if found == kind && number == schema),
                "{error:?}"
            );
            assert_eq!(
                error.to_string(),
                format!("unsupported {kind} schema {schema}")
            );
            fs::write(&path, &original).unwrap();
        }
    }
}

#[test]
fn json_failures_keep_the_saved_object_kind() {
    use echode_history::{PackageError, PackageValidationError as Invalid};

    let fixture = Fixture::new();
    for (kind, path) in fixture.files() {
        let original = fs::read_to_string(&path).unwrap();
        let mut unknown: serde_json::Value = serde_json::from_str(&original).unwrap();
        unknown["unexpected"] = serde_json::json!(true);
        for text in ["{".to_owned(), unknown.to_string()] {
            let error = fixture.reject_file(&path, &text);
            assert!(
                matches!(&error, PackageError::Validation(Invalid::Json { kind: found, .. }) if *found == kind)
            );
            assert!(cause::<serde_json::Error>(&error).is_some(), "{error:?}");
            fs::write(&path, &original).unwrap();
        }
    }
}

#[test]
fn legacy_migration_refusals_keep_the_failing_step_and_shape() {
    use echode_history::{
        MigrationError, PackageError, PackageKind, PackageValidationError as Invalid,
    };
    use serde_json::json;

    let fixture = Fixture::new();
    let (_, path) = &fixture.files()[1];
    let original = fs::read_to_string(path).unwrap();
    for schema in 1..6 {
        for (devices, expected) in [
            (json!(null), MigrationError::DevicesNotArray),
            (json!([false]), MigrationError::DeviceNotObject),
        ] {
            let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
            value["schema"] = json!(schema);
            value["devices"] = devices;
            let error = fixture.reject_file(path, &value.to_string());
            let step = if schema == 2 { 3 } else { schema };
            assert!(
                matches!(error, PackageError::Validation(Invalid::Migration {
                kind: PackageKind::Track, schema: found, error: reason,
            }) if found == step && reason == expected),
                "{error:?}"
            );
            assert_eq!(cause::<MigrationError>(&error), Some(&expected));
            assert_eq!(
                error.to_string(),
                format!("track schema {step}: {expected}")
            );
            fs::write(path, &original).unwrap();
        }
    }
}

#[test]
fn package_semantic_refusals_are_not_json_failures() {
    use echode_history::{PackageError, PackageKind, PackageValidationError as Invalid};
    use serde_json::json;

    let fixture = Fixture::new();
    let files = fixture.files();
    for (index, field, wrong, expected) in [
        (0, "tempo", json!([]), Invalid::InvalidTempoMap),
        (
            0,
            "tracks",
            json!(["../escape"]),
            Invalid::InvalidListedId("../escape".into()),
        ),
        (
            0,
            "contents",
            json!(["../escape"]),
            Invalid::InvalidListedId("../escape".into()),
        ),
        (
            1,
            "id",
            json!(Id::generate().to_string()),
            Invalid::IdMismatch {
                kind: PackageKind::Track,
                id: String::new(),
            },
        ),
        (
            2,
            "id",
            json!(Id::generate().to_string()),
            Invalid::IdMismatch {
                kind: PackageKind::Content,
                id: String::new(),
            },
        ),
    ] {
        let path = &files[index].1;
        let original = fs::read_to_string(path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        value[field] = wrong;
        let error = fixture.reject_file(path, &value.to_string());
        let PackageError::Validation(actual) = &error else {
            panic!("{error:?}")
        };
        assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(&expected)
        );
        assert!(cause::<serde_json::Error>(&error).is_none());
        fs::write(path, original).unwrap();
    }
}
