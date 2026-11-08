//! Native deadline fixtures. This executable is never included in Echode.app.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const PLUGIN_LINE: &str = "{\"id\":\"dev.audit.scan\",\"name\":\"Scan\",\"vendor\":\"Echode\",\"version\":\"1\",\"features\":[],\"url\":\"\"}\n";
const SLOW_LINE: &str = "{\"id\":\"dev.audit.slow\",\"name\":\"Slow\",\"vendor\":\"Echode\",\"version\":\"1\",\"features\":[],\"url\":\"\"}\n";

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().expect("fixture command");
    if command == "--hold-output" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    assert_eq!(command, "--scan");
    let bundle = args.next().expect("fixture bundle");
    // The live catalog passes a folder's full path; the mode is the bundle's name.
    let mode = Path::new(&bundle)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("fixture mode");
    match mode {
        "exit.clap" => std::process::exit(23),
        "signal.clap" => {
            // SAFETY: only this disposable fixture process receives SIGTERM.
            unsafe { libc::raise(libc::SIGTERM) };
            std::process::exit(1);
        }
        "hang.clap" => std::thread::sleep(Duration::from_secs(30)),
        // Longer than a test's first try and far shorter than its slow try.
        "slow.clap" => {
            std::thread::sleep(Duration::from_millis(500));
            std::io::stdout()
                .write_all(SLOW_LINE.as_bytes())
                .expect("report");
        }
        "bad.clap" => std::io::stdout().write_all(&[0xff]).expect("invalid UTF-8"),
        "failed.clap" => {
            eprintln!("boom");
            std::process::exit(1);
        }
        mode @ ("bundle.clap" | "noisy.clap" | "held.clap") => {
            std::io::stdout()
                .write_all(PLUGIN_LINE.as_bytes())
                .expect("report");
            std::io::stdout().flush().expect("flush report");
            if mode == "noisy.clap" {
                // This output size only tests draining past the scan's diagnostic cap.
                std::io::stderr()
                    .write_all(&[b'x'; 80 * 1024])
                    .expect("noisy stderr");
            } else if mode == "held.clap" {
                // The descendant deliberately inherits both pipes and outlives this process.
                #[expect(
                    clippy::zombie_processes,
                    reason = "fixture exits while its descendant holds the pipes"
                )]
                let _child = Command::new(std::env::current_exe().expect("fixture executable"))
                    .arg("--hold-output")
                    .spawn()
                    .expect("pipe-holding descendant");
            }
        }
        mode => panic!("unknown scan fixture {mode}"),
    }
}
