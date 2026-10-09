use std::process::ExitCode;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_engine::GuardAlloc = echode_engine::GuardAlloc;

fn main() -> ExitCode {
    match echode_cli::run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("echode: {error}");
            ExitCode::from(2)
        }
    }
}
