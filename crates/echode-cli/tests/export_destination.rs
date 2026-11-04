use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use echode_history::{DocumentHost, write_package};
use echode_model::Project;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "echode-export-destination-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let fixture = Self(root);
        write_package(&fixture.package(), &Project::new()).unwrap();
        echode_history::write_object(&fixture.package(), b"saved plugin state").unwrap();
        drop(DocumentHost::open(&fixture.package()).unwrap());
        fixture
    }

    fn package(&self) -> PathBuf {
        self.0.join("Song.echode")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn contract_render_refuses_package_destinations_without_changing_the_source() {
    let fixture = Fixture::new();
    let package = fixture.package();
    // A rejected destination must not initialize history as a side effect.
    std::fs::remove_dir_all(package.join(".git")).unwrap();
    let original = files(&package);
    let output = Command::new(env!("CARGO_BIN_EXE_echode"))
        .arg("render")
        .arg(&package)
        .arg(package.join("project.json"))
        .args(["--frames", "1", "--progress-json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "render must refuse a destination inside its source package"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("output outside the project package"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("exported "));
    assert!(output.stdout.is_empty());
    assert_eq!(
        files(&package),
        original,
        "rejection must leave every source file unchanged"
    );
}

#[test]
fn package_aliases_and_nested_destinations_are_rejected_by_the_library() {
    let fixture = Fixture::new();
    let package = fixture.package();
    let alias = fixture.0.join("alias");
    std::os::unix::fs::symlink(&package, &alias).unwrap();
    let output_link = fixture.0.join("linked.wav");
    std::os::unix::fs::symlink(package.join("project.json"), &output_link).unwrap();
    let original = files(&package);
    for source in [&package, &alias] {
        for output in [
            package.clone(),
            package.join("new.wav"),
            package.join(".git/HEAD"),
            alias.join("new.wav"),
            fixture.0.join("alias/../Song.echode/new.wav"),
            output_link.clone(),
        ]
        .into_iter()
        .chain(original.keys().map(|path| package.join(path)))
        {
            let error = echode_cli::render_to_wav(source, &output, 48_000, Some(1)).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("output outside the project package"),
                "{error}"
            );
            assert_eq!(files(&package), original);
        }
    }
    assert!(output_link.is_symlink());
}

#[test]
fn outside_outputs_allow_replacement_relative_paths_and_sibling_prefixes() {
    let fixture = Fixture::new();
    let package = fixture.package();
    let sibling = fixture.0.join("Song.echode-exports");
    std::fs::create_dir(&sibling).unwrap();
    let alias = fixture.0.join("exports");
    std::os::unix::fs::symlink(&sibling, &alias).unwrap();
    let original = files(&package);
    for output in [
        fixture.0.join("mix.wav"),
        sibling.join("mix.wav"),
        alias.join("mix.wav"),
        package.join("../relative.wav"),
    ] {
        std::fs::write(&output, b"old export").unwrap();
        echode_cli::render_to_wav(&package, &output, 48_000, Some(1)).unwrap();
        assert_eq!(&std::fs::read(output).unwrap()[..4], b"RIFF");
        assert_eq!(files(&package), original);
    }
}

#[test]
fn relative_cli_output_and_missing_parent_do_not_change_the_source() {
    let fixture = Fixture::new();
    let package = fixture.package();
    let original = files(&package);
    let output = Command::new(env!("CARGO_BIN_EXE_echode"))
        .current_dir(&fixture.0)
        .args(["render", "Song.echode", "mix.wav", "--frames", "1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        &std::fs::read(fixture.0.join("mix.wav")).unwrap()[..4],
        b"RIFF"
    );
    let error =
        echode_cli::render_to_wav(&package, &package.join("missing/out.wav"), 48_000, Some(1))
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot resolve export output directory"),
        "{error}"
    );
    assert_eq!(files(&package), original);
    assert!(!package.join("missing").exists());
}

#[test]
fn outside_leaf_symlink_is_replaced_without_changing_its_target() {
    let fixture = Fixture::new();
    let target = fixture.0.join("keep.wav");
    std::fs::write(&target, b"keep original").unwrap();
    let link = fixture.0.join("mix.wav");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    echode_cli::render_to_wav(&fixture.package(), &link, 48_000, Some(1)).unwrap();
    assert!(!link.is_symlink());
    assert_eq!(&std::fs::read(link).unwrap()[..4], b"RIFF");
    assert_eq!(std::fs::read(target).unwrap(), b"keep original");
}
