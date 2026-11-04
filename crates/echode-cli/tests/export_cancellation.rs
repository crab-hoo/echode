use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use echode_history::write_package;
use echode_model::{Id, Project};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn closing_the_control_pipe_cancels_plain_and_isolated_exports_without_publication() {
    for plugin in [false, true] {
        let root =
            std::env::temp_dir().join(format!("echode-cancel-{}-{plugin}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let result = std::panic::catch_unwind(|| {
            let package = root.join("Song.echode");
            let mut project = Project::new();
            if plugin {
                let (next, track) = project.add_track("Sine Pad");
                project = next
                    .add_clap_device(track, Id::generate(), "dev.echode.sine-pad")
                    .unwrap();
            }
            write_package(&package, &project).unwrap();
            let output = root.join("original.wav");
            std::fs::write(&output, b"keep original").unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_echode"));
            command
                .args(["render"])
                .arg(&package)
                .arg(&output)
                // A deliberately long export leaves time for the control handshake, not a product limit.
                .args([
                    "--frames",
                    "4294967295",
                    "--progress-json",
                    "--cancel-stdin",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if plugin {
                let binaries = Path::new(env!("CARGO_BIN_EXE_echode")).parent().unwrap();
                let cache = root.join("plugins.jsonl");
                let entry = serde_json::json!({
                    "path": binaries.join("libechode_sine_pad.dylib"),
                    "bundle_version": "", "modified_secs": 0, "modified_nanos": 0, "executable_size": 0,
                    "plugins": [{"id": "dev.echode.sine-pad", "name": "Sine Pad", "vendor": "Echode",
                        "version": "0.0.0", "features": ["instrument"], "url": ""}]
                });
                std::fs::write(&cache, format!("{entry}\n")).unwrap();
                command
                    .env("ECHODE_PLUGIN_HOST", binaries.join("echode-plugin-host"))
                    .env("ECHODE_PLUGIN_CACHE", cache);
            }
            let mut child = Running(command.spawn().unwrap());
            let stdout = child.0.stdout.take().unwrap();
            let (send, receive) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if send.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
            let initial = receive
                .recv_timeout(Duration::from_secs(10))
                .expect("cancellable export did not start");
            assert!(initial.contains("\"phase\":\"rendering\""));
            drop(child.0.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "export did not acknowledge cancellation"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            reader.join().unwrap();
            let stderr = std::io::read_to_string(child.0.stderr.take().unwrap()).unwrap();
            assert_eq!(status.code(), Some(130), "cancellation status: {stderr}");
            assert!(stderr.contains("export cancelled") && !stderr.contains("exported "));
            assert!(!receive.try_iter().any(|line| line.contains("\"complete\"")));
            assert_eq!(std::fs::read(&output).unwrap(), b"keep original");
            assert!(!std::fs::read_dir(&root).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".part")
            }));
        });
        std::fs::remove_dir_all(root).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}

#[test]
fn an_open_control_pipe_allows_success_and_default_stdin_eof_is_not_cancellation() {
    let root = std::env::temp_dir().join(format!("echode-cancel-success-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let package = root.join("Song.echode");
    write_package(&package, &Project::new()).unwrap();
    for cancellable in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_echode"));
        command
            .arg("render")
            .arg(&package)
            .arg(root.join("out.wav"))
            .args(["--frames", "65"]);
        if cancellable {
            command.arg("--cancel-stdin").stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = Running(command.spawn().unwrap());
        let _control = child.0.stdin.take();
        assert!(child.0.wait().unwrap().success());
    }
    std::fs::remove_dir_all(root).unwrap();
}
