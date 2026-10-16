use echode_engine::spike::{install_stop_handler, run, run_at_device_rate};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let mut seconds = 600u64;
    let mut workers = None;
    let mut report_path = None;
    let mut native_rate = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--native-rate" => native_rate = true,
            "--seconds" => {
                let Some(value) = args.next().and_then(|text| text.parse().ok()) else {
                    eprintln!("--seconds needs a number");
                    return ExitCode::from(2);
                };
                seconds = value;
            }
            "--workers" => {
                let Some(text) = args.next() else {
                    eprintln!("--workers needs a number or auto");
                    return ExitCode::from(2);
                };
                if text == "auto" {
                    workers = None;
                } else if let Ok(value) = text.parse() {
                    workers = Some(value);
                } else {
                    eprintln!("--workers needs a number or auto");
                    return ExitCode::from(2);
                }
            }
            "--report" => {
                let Some(path) = args.next() else {
                    eprintln!("--report needs a path");
                    return ExitCode::from(2);
                };
                report_path = Some(PathBuf::from(path));
            }
            "--help" | "-h" => {
                eprintln!(
                    "usage: echode-engine-spike [--seconds N] [--workers N|auto] [--report PATH] [--native-rate]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }

    install_stop_handler();
    let measure = if native_rate { run_at_device_rate } else { run };
    match measure(seconds, workers) {
        Ok(report) => {
            print!("{report}");
            if let Some(path) = report_path {
                if let Some(parent) = path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(error) = fs::write(&path, report.to_string()) {
                    eprintln!("could not write {}: {error}", path.display());
                    return ExitCode::FAILURE;
                }
            }
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("echode-engine-spike: {error}");
            ExitCode::FAILURE
        }
    }
}
