//! Cross-process hosting: audio actually crosses, a dead plugin does not kill us,
//! and the round trip is measured against the Phase 0 budget.

use echode_plugin_host::measure_in_process;
use echode_sandbox::{PluginHost, SessionError, SpawnOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_micros(30);
const WARMUP: usize = 200;
const SAMPLES: usize = 2_000;

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let host = host_bin();
    let sibling = host.parent().unwrap().join("libechode_test_plugin.dylib");
    if sibling.exists() {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if sibling.exists() {
        return sibling;
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-plugin");
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg("build")
        .arg("-p")
        .arg("echode-test-plugin")
        .arg("--target-dir")
        .arg(&target);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command.status().expect("spawn cargo for the test plugin");
    assert!(status.success(), "failed to build echode-test-plugin");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    target.join(profile).join("libechode_test_plugin.dylib")
}

fn spawn(plugin: Option<&Path>, fault: Option<&str>) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        host_bin: &host_bin(),
        plugin,
        fault,
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
}

fn outs(output: &mut [Vec<f32>; 2]) -> [&mut [f32]; 2] {
    let (left, right) = output.split_at_mut(1);
    [&mut left[0], &mut right[0]]
}

fn stereo(frames: usize, seed: f32) -> [Vec<f32>; 2] {
    let left: Vec<f32> = (0..frames).map(|i| seed + i as f32 * 0.001).collect();
    let right: Vec<f32> = (0..frames).map(|i| -seed + i as f32 * 0.002).collect();
    [left, right]
}

struct Stats {
    min: Duration,
    p50: Duration,
    p99: Duration,
    max: Duration,
}

fn measure(host: &mut PluginHost, frames: usize) -> Stats {
    let input = stereo(frames, 0.1);
    let mut output = [vec![0.0; frames], vec![0.0; frames]];
    for _ in 0..WARMUP {
        let planes: [&[f32]; 2] = [&input[0], &input[1]];
        host.process(&planes, &mut outs(&mut output), Duration::from_millis(100))
            .expect("warmup block");
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let planes: [&[f32]; 2] = [&input[0], &input[1]];
        host.process(&planes, &mut outs(&mut output), Duration::from_millis(100))
            .expect("measured block");
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    Stats {
        min: samples[0],
        p50: samples[SAMPLES / 2],
        p99: samples[(SAMPLES * 99) / 100],
        max: samples[SAMPLES - 1],
    }
}

fn us(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

#[test]
fn copy_roundtrip_preserves_samples() {
    let mut host = spawn(None, None);
    let input = stereo(128, 0.25);
    let mut output = [vec![0.0; 128], vec![0.0; 128]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("copy");
    assert_eq!(output[0], input[0]);
    assert_eq!(output[1], input[1]);
}

#[test]
fn clap_gain_doubles_samples_and_stays_within_budget() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), None);
    let frames = 64;
    let input = stereo(frames, -0.4);
    let mut output = [vec![0.0; frames], vec![0.0; frames]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("gain");
    for channel in 0..2 {
        for (got, source) in output[channel].iter().zip(&input[channel]) {
            let expected = source * 2.0;
            assert!(
                (got - expected).abs() < 1.0e-6,
                "channel {channel}: got {got}, expected {expected}"
            );
        }
    }

    let isolated_64 = measure(&mut host, frames);
    let isolated_128 = measure(&mut host, 128);
    let mut copy_host = spawn(None, None);
    let copy_64 = measure(&mut copy_host, frames);
    let in_process = measure_in_process(&plugin, frames, SAMPLES).expect("in-process clap");
    let in_process_each = in_process / u32::try_from(SAMPLES).unwrap();
    let report = format!(
        "samples: {SAMPLES} after {WARMUP} warmup, budget p50 <= {budget:.0} µs\n\
         copy 64:       min {cmin:.1}  p50 {cp50:.1}  p99 {cp99:.1}  max {cmax:.1} µs\n\
         CLAP 64:       min {min:.1}  p50 {p50:.1}  p99 {p99:.1}  max {max:.1} µs\n\
         CLAP 128:      min {bmin:.1}  p50 {bp50:.1}  p99 {bp99:.1}  max {bmax:.1} µs\n\
         in-process 64: mean {local:.1} µs\n",
        budget = us(BUDGET),
        cmin = us(copy_64.min),
        cp50 = us(copy_64.p50),
        cp99 = us(copy_64.p99),
        cmax = us(copy_64.max),
        min = us(isolated_64.min),
        p50 = us(isolated_64.p50),
        p99 = us(isolated_64.p99),
        max = us(isolated_64.max),
        bmin = us(isolated_128.min),
        bp50 = us(isolated_128.p50),
        bp99 = us(isolated_128.p99),
        bmax = us(isolated_128.max),
        local = us(in_process_each),
    );
    let report_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-s3.txt");
    std::fs::write(&report_path, &report).expect("write spike report");
    println!("{report}");

    // Debug builds are not the measurement. The budget is for release.
    if !cfg!(debug_assertions) {
        assert!(
            isolated_64.p50 <= BUDGET,
            "median round trip {} µs exceeds {} µs",
            us(isolated_64.p50),
            us(BUDGET)
        );
    }
}

#[test]
fn plugin_crash_does_not_kill_the_host() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("crash"));
    let input = stereo(32, 0.0);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    let error = host
        .process(&planes, &mut outs(&mut output), Duration::from_millis(500))
        .expect_err("crash should fail the block");
    match error {
        SessionError::HostExited { status, .. } => {
            let signal = std::os::unix::process::ExitStatusExt::signal(&status);
            assert_eq!(signal, Some(6), "expected SIGABRT, got {status}");
        }
        other => panic!("expected the plugin process to abort, got {other}"),
    }
    drop(host);

    let mut again = spawn(Some(&plugin), None);
    again
        .process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("a new plugin process works after the previous one crashed");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
}

#[test]
fn plugin_hang_times_out_and_can_be_replaced() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("hang"));
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    let error = host
        .process(&planes, &mut outs(&mut output), Duration::from_millis(100))
        .expect_err("hang should time out");
    assert!(
        matches!(error, SessionError::Timeout),
        "expected timeout, got {error}"
    );
    drop(host);

    let mut again = spawn(Some(&plugin), None);
    again
        .process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("hosting continues after a hung plugin is killed");
}
