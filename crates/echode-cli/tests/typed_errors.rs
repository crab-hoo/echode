use std::error::Error;
use std::path::PathBuf;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        // Keep Unix socket fixtures below macOS's sockaddr_un path limit.
        let root = PathBuf::from("/tmp").join(format!(
            "echode-cli-errors-{}",
            echode_model::Id::generate()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn render_path_failure_keeps_errno_without_touching_the_output() {
    let fixture = Fixture::new();
    let output = fixture.0.join("mix.wav");
    std::fs::write(&output, b"original").unwrap();
    let error = echode_cli::render_to_wav(&fixture.0.join("missing"), &output, 48_000, Some(1))
        .unwrap_err();
    assert!(matches!(&error, echode_cli::CliError::Io {
        operation: echode_cli::CliIoOperation::ResolvePackage, path: Some(path), ..
    } if path == &fixture.0.join("missing")));
    assert!(
        error
            .to_string()
            .starts_with("cannot resolve project package: ")
    );
    let cause = error
        .source()
        .and_then(|cause| cause.downcast_ref::<std::io::Error>());
    assert!(
        cause.is_some(),
        "CLI path resolution must retain the I/O cause"
    );
    assert_eq!(cause.unwrap().raw_os_error(), Some(2));
    assert_eq!(std::fs::read(&output).unwrap(), b"original");
}

#[test]
fn integer_argument_keeps_the_parse_cause_and_original_diagnostic() {
    let error = echode_cli::run(["render", "unused", "unused.wav", "--frames", "-1"]).unwrap_err();
    assert_eq!(error.to_string(), "expected a non-negative integer, got -1");
    assert!(
        error
            .source()
            .and_then(Error::source)
            .is_some_and(|cause| cause.is::<std::num::ParseIntError>()),
        "CLI integer parsing must retain its parse cause"
    );
}

#[test]
fn wav_create_failure_keeps_the_io_cause_and_existing_files() {
    let fixture = Fixture::new();
    let output = fixture.0.join("missing/mix.wav");
    let error = echode_cli::write_wav(&output, 48_000, &[0.0, 0.0]).unwrap_err();
    assert!(matches!(&error, echode_cli::CliError::Io {
        operation: echode_cli::CliIoOperation::CreateWav, path: Some(path), ..
    } if path.parent() == output.parent()));
    assert!(
        error
            .source()
            .is_some_and(|cause| cause.is::<std::io::Error>()),
        "CLI WAV creation must retain the I/O cause"
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
}

#[test]
fn serve_keeps_api_and_package_causes_and_does_not_create_a_package() {
    let fixture = Fixture::new();
    let package = fixture.0.join("missing");
    let socket = fixture.0.join("session.sock");
    let error = echode_cli::run(["serve", package.to_str().unwrap(), socket.to_str().unwrap()])
        .unwrap_err();
    let api = error
        .source()
        .unwrap()
        .downcast_ref::<echode_api::ApiError>()
        .unwrap();
    assert!(matches!(api, echode_api::ApiError::Package(_)));
    let package_error = api
        .source()
        .unwrap()
        .downcast_ref::<echode_history::PackageError>()
        .unwrap();
    assert_eq!(error.to_string(), package_error.to_string());
    assert!(!package.exists());
}

#[test]
fn render_keeps_package_validation_cause_without_replacing_the_output() {
    let fixture = Fixture::new();
    let package = fixture.0.join("Song.echode");
    echode_history::write_package(&package, &echode_model::Project::new()).unwrap();
    std::fs::write(package.join("project.json"), b"broken json").unwrap();
    let output = fixture.0.join("mix.wav");
    std::fs::write(&output, b"original").unwrap();
    let error = echode_cli::render_to_wav(&package, &output, 48_000, Some(1)).unwrap_err();
    let cause = error
        .source()
        .unwrap()
        .downcast_ref::<echode_history::PackageError>()
        .unwrap();
    assert!(matches!(cause, echode_history::PackageError::Validation(_)));
    assert_eq!(error.to_string(), cause.to_string());
    assert_eq!(std::fs::read(&output).unwrap(), b"original");
    assert_eq!(
        std::fs::read(package.join("project.json")).unwrap(),
        b"broken json"
    );
}

#[test]
fn executable_keeps_invalid_argument_exit_code_and_stderr() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_echode"))
        .args(["render", "unused", "unused.wav", "--frames", "-1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"echode: expected a non-negative integer, got -1\n"
    );
}
