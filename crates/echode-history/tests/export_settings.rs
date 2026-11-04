//! Export preferences are portable convenience data, not project history.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use echode_history::{Autosave, read_export_settings, write_export_settings, write_package};
use echode_model::{Id, Project};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let package =
            std::env::temp_dir().join(format!("echode-export-settings-{}", Id::generate()));
        write_package(&package, &Project::new()).unwrap();
        Self(package)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preferences_survive_model_writes_but_are_not_in_autosave_trees() {
    let fixture = Fixture::new();
    assert_eq!(read_export_settings(&fixture.0), None);
    let text = r#"{"version":1,"settings":{"format":"pcm16"}}"#;
    write_export_settings(&fixture.0, text).unwrap();
    let project = Arc::new(Project::new().add_track("later edit").0);
    let autosave = Autosave::create(&fixture.0).unwrap();
    let revision = autosave.save(project.clone(), 0).unwrap().wait().unwrap();
    assert_eq!(read_export_settings(&fixture.0).as_deref(), Some(text));
    assert_eq!(autosave.load(revision).unwrap(), *project);
    let repo = gix::open(&fixture.0).unwrap();
    let tree = repo.find_commit(revision).unwrap().tree().unwrap();
    assert!(tree.find_entry("export.json").is_none());
}

#[test]
fn malformed_oversized_and_nonregular_entries_cannot_escape_the_package() {
    let fixture = Fixture::new();
    let entry = fixture.0.join("export.json");
    let original = r#"{"version":1}"#;
    write_export_settings(&fixture.0, original).unwrap();
    // The limit is for this preference record, not for the size of a project.
    let oversized = format!(r#"{{"padding":"{}"}}"#, "x".repeat(16_384));
    for invalid in ["not JSON", "[]", "null", &oversized] {
        assert!(write_export_settings(&fixture.0, invalid).is_err());
        assert_eq!(fs::read_to_string(&entry).unwrap(), original);
        fs::write(&entry, invalid).unwrap();
        assert_eq!(read_export_settings(&fixture.0), None);
        write_export_settings(&fixture.0, original).unwrap();
    }
    fs::write(&entry, [0xff, 0xfe]).unwrap();
    assert_eq!(read_export_settings(&fixture.0), None);
    fs::remove_file(&entry).unwrap();
    let outside = fixture.0.join("outside.json");
    fs::write(&outside, original).unwrap();
    std::os::unix::fs::symlink(&outside, &entry).unwrap();
    assert_eq!(read_export_settings(&fixture.0), None);
    assert!(write_export_settings(&fixture.0, "{}").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), original);
    fs::remove_file(&entry).unwrap();
    fs::create_dir(&entry).unwrap();
    assert_eq!(read_export_settings(&fixture.0), None);
    assert!(write_export_settings(&fixture.0, "{}").is_err());
}
