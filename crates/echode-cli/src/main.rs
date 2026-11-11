use std::process::ExitCode;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;

fn main() -> ExitCode {
    // An audio callback's panic reaches the command's error with its location
    // (ADR 0238). The CLI keeps no log file; stderr already shows other panics.
    echode_audio_io::keep_panic_log(None);
    // The unified log only (ADR 0322): stderr stays the command's own report,
    // and `log stream --predicate 'subsystem == "io.echode"'` shows the rest.
    echode_log::install(echode_log::Options {
        os_log: true,
        ..echode_log::Options::default()
    });
    match echode_cli::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("echode: {error}");
            ExitCode::from(if error.is_cancelled() { 130 } else { 2 })
        }
    }
}
