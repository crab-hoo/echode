use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut shm = None;
    let mut plugin = None;
    let mut scan = None;
    let mut workgroup_name = None;
    let mut workgroup_ticks = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shm" => shm = args.next(),
            "--plugin" => plugin = args.next(),
            "--scan" => scan = args.next(),
            "--workgroup-name" => workgroup_name = args.next(),
            "--workgroup-ticks" => {
                let Some(value) = args.next().and_then(|text| text.parse().ok()) else {
                    eprintln!("--workgroup-ticks needs a number");
                    return ExitCode::from(2);
                };
                workgroup_ticks = Some(value);
            }
            "--help" | "-h" => {
                eprintln!(
                    "usage: echode-plugin-host --shm PATH [--plugin PATH]\n       echode-plugin-host --scan PATH"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    if let Some(scan) = scan {
        return match echode_plugin_host::scan_descriptors(Path::new(&scan)) {
            Ok(plugins) => {
                print!("{}", echode_plugin_host::scan_report(&plugins));
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("echode-plugin-host: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some(shm) = shm else {
        eprintln!(
            "usage: echode-plugin-host --shm PATH [--plugin PATH]\n       echode-plugin-host --scan PATH"
        );
        return ExitCode::from(2);
    };
    let workgroup = match (workgroup_name, workgroup_ticks) {
        (Some(name), Some(ticks)) => Some((name, ticks)),
        (None, None) => None,
        _ => {
            eprintln!("--workgroup-name and --workgroup-ticks are set together");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = echode_plugin_host::serve(
        Path::new(&shm),
        plugin.as_deref().map(Path::new),
        workgroup
            .as_ref()
            .map(|(name, ticks)| (name.as_str(), *ticks)),
    ) {
        eprintln!("echode-plugin-host: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
