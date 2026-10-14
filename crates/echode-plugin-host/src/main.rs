use std::path::Path;
use std::process::ExitCode;

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: echode_plugin_host::GuardAlloc = echode_plugin_host::GuardAlloc;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut shm = None;
    let mut shm_fd = None;
    let mut control_fd = None;
    let mut fan_fd = None;
    let mut plugin = None;
    let mut scan = None;
    let mut workgroup_name = None;
    let mut workgroup_ticks = None;
    let mut plugin_id = None;
    let mut sample_rate = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shm" => shm = args.next(),
            "--shm-fd" => shm_fd = args.next().and_then(|text| text.parse().ok()),
            "--control-fd" => control_fd = args.next().and_then(|text| text.parse().ok()),
            "--fan-fd" => fan_fd = args.next().and_then(|text| text.parse().ok()),
            "--plugin" => plugin = args.next(),
            "--plugin-id" => plugin_id = args.next(),
            "--sample-rate" => {
                let Some(value) = args.next().and_then(|text| text.parse().ok()) else {
                    eprintln!("--sample-rate needs a number");
                    return ExitCode::from(2);
                };
                sample_rate = Some(value);
            }
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
                    "usage: echode-plugin-host --shm-fd N --control-fd N [--plugin PATH]\n       echode-plugin-host --scan PATH"
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
        return match echode_plugin_host::scan_path(Path::new(&scan)) {
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
    let workgroup = match (workgroup_name, workgroup_ticks) {
        (Some(name), Some(ticks)) => Some((name, ticks)),
        (None, None) => None,
        _ => {
            eprintln!("--workgroup-name and --workgroup-ticks are set together");
            return ExitCode::from(2);
        }
    };
    let plugin = plugin.as_deref().map(Path::new);
    let plugin_id = plugin_id.as_deref();
    let sample_rate = sample_rate.unwrap_or(48_000.0);
    let workgroup = workgroup
        .as_ref()
        .map(|(name, ticks)| (name.as_str(), *ticks));
    let served = if let (Some(shm_fd), Some(control_fd)) = (shm_fd, control_fd) {
        echode_plugin_host::serve_fds(
            shm_fd,
            control_fd,
            fan_fd,
            plugin,
            plugin_id,
            sample_rate,
            workgroup,
        )
    } else if let Some(shm) = shm {
        echode_plugin_host::serve(Path::new(&shm), plugin, plugin_id, sample_rate, workgroup)
    } else {
        eprintln!(
            "usage: echode-plugin-host --shm-fd N --control-fd N [--plugin PATH]\n       echode-plugin-host --scan PATH"
        );
        return ExitCode::from(2);
    };
    if let Err(error) = served {
        eprintln!("echode-plugin-host: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
