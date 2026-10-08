use std::process::ExitCode;

fn main() -> ExitCode {
    match echode_cli::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("echode: {error}");
            ExitCode::from(2)
        }
    }
}
