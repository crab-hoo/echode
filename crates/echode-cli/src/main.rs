use std::process::ExitCode;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;

fn main() -> ExitCode {
    // An audio callback's panic reaches the command's error with its location
    // (ADR 0238). The CLI keeps no log file; stderr already shows other panics.
    echode_audio_io::keep_panic_log(None);
    match echode_cli::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("echode: {error}");
            ExitCode::from(2)
        }
    }
}
