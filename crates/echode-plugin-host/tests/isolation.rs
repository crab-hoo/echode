//! Cross-process hosting: audio actually crosses, a dead plugin does not kill us,
//! and the round trip is measured against the Phase 0 budget.

use echode_engine::IoWorkgroup;
use echode_plugin_host::measure_in_process;
use echode_realtime::{Event, NoteEvent, Target};
use echode_sandbox::{
    BlockProgress, ControlSendError, Delivered, FanIn, LaunchFailure, LaunchFailureKind,
    PluginHost, PluginNotice, Refusal, SessionError, SessionLayout, SpawnOptions, WorkgroupHandoff,
    WorkgroupMembership, copy_workgroup_port, join_workgroup,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_micros(30);
const WARMUP: usize = 200;
const SAMPLES: usize = 2_000;
/// From `abort` to exit includes the macOS crash report. A debug helper took
/// 120 to 420 ms run alone and over a second beside the other tests.
const CRASH_DEADLINE: Duration = Duration::from_secs(20);

fn host_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_echode-plugin-host"))
}

fn plugin_dylib() -> PathBuf {
    static ARTIFACT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ARTIFACT.get_or_init(build_plugin_dylib).clone()
}

fn build_plugin_dylib() -> PathBuf {
    static BUILD: Mutex<()> = Mutex::new(());
    let host = host_bin();
    let sibling = host.parent().unwrap().join("libechode_test_plugin.dylib");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../echode-test-plugin/src");
    let current = |path: &Path| {
        let Ok(built) = path.metadata().and_then(|meta| meta.modified()) else {
            return false;
        };
        let Ok(edited) = newest_source(&source) else {
            return false;
        };
        built >= edited
    };
    if current(&sibling) {
        return sibling;
    }
    let _guard = BUILD.lock().expect("plugin build lock");
    if current(&sibling) {
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
    spawn_with(plugin, fault, None)
}

fn spawn_with(
    plugin: Option<&Path>,
    fault: Option<&str>,
    workgroup: Option<WorkgroupHandoff>,
) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin,
        fault,
        workgroup,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 0,
        plugin_id: None,
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
}

fn handoff(workgroup: &IoWorkgroup) -> WorkgroupHandoff {
    WorkgroupHandoff {
        // SAFETY: `workgroup` retains the object until this test returns.
        port: unsafe { copy_workgroup_port(workgroup.as_ptr()) }
            .unwrap_or_else(|error| panic!("copy workgroup port: {error}")),
        period_ticks: workgroup.period_ticks(),
    }
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
fn clap_gain_doubles_samples() {
    let plugin = plugin_dylib();
    let mut host = spawn_with(Some(&plugin), None, None);
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
}

#[test]
#[ignore = "needs a CoreAudio device and the release timing budget; `make spike-isolation` runs it"]
fn clap_gain_doubles_samples_and_stays_within_budget() {
    let workgroup = IoWorkgroup::open().unwrap_or_else(|error| panic!("device workgroup: {error}"));
    let plugin = plugin_dylib();
    let mut host = spawn_with(Some(&plugin), None, Some(handoff(&workgroup)));
    // SAFETY: `workgroup` is dropped after `parent`, so the object outlives the join.
    let parent: WorkgroupMembership =
        unsafe { join_workgroup(workgroup.as_ptr(), workgroup.period_ticks()) }
            .unwrap_or_else(|error| panic!("parent join: {error}"));
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
    drop(host);
    let mut copy_host = spawn_with(None, None, Some(handoff(&workgroup)));
    let copy_64 = measure(&mut copy_host, frames);
    let in_process = measure_in_process(&plugin, frames, SAMPLES).expect("in-process clap");
    let in_process_each = in_process / u32::try_from(SAMPLES).unwrap();
    let report = format!(
        "samples: {SAMPLES} after {WARMUP} warmup, budget p50 <= {budget:.0} µs\n\
         workgroup: device IO {rate:.0} Hz, {frames} frames, period {period:.3} ms, parent and helper joined\n\
         copy 64:       min {cmin:.1}  p50 {cp50:.1}  p99 {cp99:.1}  max {cmax:.1} µs\n\
         CLAP 64:       min {min:.1}  p50 {p50:.1}  p99 {p99:.1}  max {max:.1} µs\n\
         CLAP 128:      min {bmin:.1}  p50 {bp50:.1}  p99 {bp99:.1}  max {bmax:.1} µs\n\
         in-process 64: mean {local:.1} µs\n",
        budget = us(BUDGET),
        rate = workgroup.sample_rate(),
        frames = workgroup.frames(),
        period = workgroup.period_ns() as f64 / 1_000_000.0,
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
    println!("{report}");

    // Debug builds are not the measurement. The report and the budget are for release.
    if !cfg!(debug_assertions) {
        let report_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-s3.txt");
        std::fs::write(&report_path, &report).expect("write spike report");
        assert!(
            isolated_64.p50 <= BUDGET,
            "median round trip {} µs exceeds {} µs",
            us(isolated_64.p50),
            us(BUDGET)
        );
    }
    drop(parent);
}

#[test]
fn plugin_crash_does_not_kill_the_host() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("crash"));
    let input = stereo(32, 0.0);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    let error = host
        .process(&planes, &mut outs(&mut output), CRASH_DEADLINE)
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
fn plugin_segfault_marks_the_mapping_before_exit() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("segfault"));
    let input = stereo(32, 0.0);
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.submit(32, &planes, &[])
        .expect("publish the block that faults");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !host.fatal_notice() {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        host.fatal_notice(),
        "SIGSEGV should store fail_len 2 before the process is reaped"
    );
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

#[test]
fn a_restart_while_idle_wakes_the_processing_thread() {
    let mut host = spawn(None, Some("restart-idle"));
    let started = Instant::now();
    while !host.stderr_log().contains("restart-observed") {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "restart while idle was not observed: {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("a block after an idle restart");
    assert_eq!(output[0], input[0]);
}

#[test]
fn a_callback_runs_on_the_main_thread_and_a_restart_still_answers() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("callback"));
    let input = stereo(32, 0.3);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("callback block");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
    let started = Instant::now();
    while !host.stderr_log().contains("on-main-thread") {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "main-thread callback was not delivered: {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut host = spawn(Some(&plugin), Some("restart"));
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the block that requested a restart still returns");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the next block is answered without waiting out the deadline");
    let started = Instant::now();
    loop {
        if matches!(
            host.poll_notice().expect("notice"),
            Some(PluginNotice::PluginRestarted { .. })
        ) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "restart notice did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_plugin(fault: Option<&str>, layout: SessionLayout) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin_dylib()),
        fault,
        workgroup: None,
        layout,
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some("dev.echode.test-gain"),
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
}

fn spawn_layout(layout: SessionLayout) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: None,
        fault: None,
        workgroup: None,
        layout,
        fan_in: None,
        sample_rate: 0,
        plugin_id: None,
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
}

#[test]
fn reset_clears_delay_state_under_rtsan() {
    for fault in ["tail", "tail-legacy"] {
        let mut host = spawn_plugin(Some(fault), SessionLayout::stereo());
        let mut input = stereo(64, 0.25); // Only this fixture's block/channel counts.
        let mut output = stereo(64, 0.0);
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .unwrap();
        input.iter_mut().for_each(|plane| plane.fill(0.0));
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        host.request_reset();
        host.process(
            &[&input[0], &input[1]],
            &mut outs(&mut output),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(output.iter().flatten().all(|sample| *sample == 0.0));
    }
}

#[test]
fn audit_pro08_cold_process_under_rtsan() {
    let mut host = spawn_plugin(None, SessionLayout::stereo());
    let input = stereo(32, 0.25);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the first block allocates nothing");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
}

#[test]
fn audit_pro08_event_growth_under_rtsan() {
    let mut host = spawn_plugin(
        Some("count"),
        SessionLayout {
            max_frames: 64,
            input_channels: 2,
            output_channels: 2,
            input_events: 1,
            output_events: 1,
        },
    );
    let input = stereo(16, 0.0);
    let mut output = [vec![0.0; 16], vec![0.0; 16]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    let event = note();
    host.process_events(
        &planes,
        &mut outs(&mut output),
        &[event],
        &mut [],
        Duration::from_secs(2),
    )
    .expect("one note fits the initial buffer");
    assert_eq!(output[0][0], 1.0);
    host.remap(SessionLayout {
        max_frames: 64,
        input_channels: 2,
        output_channels: 2,
        input_events: 8,
        output_events: 8,
    })
    .expect("the control thread grows the buffers");
    host.process_events(
        &planes,
        &mut outs(&mut output),
        &[event, event, event, event],
        &mut [],
        Duration::from_secs(2),
    )
    .expect("a larger note list allocates nothing on the audio thread");
    assert_eq!(output[0][0], 5.0, "all four note-ons reach the plugin");
}

#[test]
fn audit_pro08_restart_under_rtsan() {
    let mut host = spawn_plugin(Some("restart"), SessionLayout::stereo());
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the block that requests a restart");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
    let started = Instant::now();
    loop {
        if matches!(
            host.poll_notice().expect("notice"),
            Some(PluginNotice::PluginRestarted { .. })
        ) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "restart did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("restart hands the processor over without allocating");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
}

/// RTA-02. The plugin asks for a restart in its first block and again from
/// the activation that restart runs, which then takes 50 ms to return. The
/// processing thread acknowledges the second request while it holds no
/// processor, then starts the processor the first restart activated. The
/// control thread waits for that processor to be released, which nothing
/// asks for any more, and no longer reads its socket.
#[test]
#[ignore = "RTA-02: a restart requested while restarting leaves the helper's control thread waiting"]
fn audit_rta02_a_restart_requested_while_restarting_is_run_and_the_helper_keeps_answering() {
    let mut host = spawn_plugin(Some("restart-nested"), SessionLayout::stereo());
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the block that requests the first restart");
    // Each activation prints its number; the second one asks again.
    for activation in ["nested-activation 2", "nested-activation 3"] {
        let started = Instant::now();
        while !host.stderr_log().contains(activation) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "`{activation}` never ran: {}",
                host.stderr_log()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // Audio crosses either way, so only a control request shows a stuck helper.
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("a block after the restarts");
    host.set_chain(1)
        .expect("the control thread still answers after the nested restart");
}

/// RTA-01. `request_callback` is thread-safe in CLAP and plugins call it from
/// `process`. The helper answers by signalling its main run loop there, which
/// takes CoreFoundation's locks on the processing thread. RTSan stops the
/// helper, so the block is not answered. Only an RTSan build can see it:
/// `RTSAN_ENABLE=1 cargo test -p echode-plugin-host --features rtsan --test isolation -- --ignored audit_rta01`.
#[cfg(feature = "rtsan")]
#[test]
#[ignore = "RTA-01: request_callback in process signals the run loop on the processing thread"]
fn audit_rta01_a_callback_requested_in_process_under_rtsan() {
    let mut host = spawn_plugin(Some("callback"), SessionLayout::stereo());
    let input = stereo(32, 0.3);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the block that requests a main-thread callback");
    let started = Instant::now();
    while !host.stderr_log().contains("on-main-thread") {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "main-thread callback was not delivered: {}",
            host.stderr_log()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// RTA-03. The callback may publish a block while the helper is still inside
/// an earlier one it gave up waiting for, so that the plugin still hears the
/// later block's events. When a prepared mapping is activated meanwhile, the
/// helper moves to it as soon as the earlier block returns and never
/// processes the block already published on the old mapping.
#[test]
#[ignore = "RTA-03: a block published before a mapping change is never processed"]
fn audit_rta03_a_block_published_before_a_mapping_change_is_processed() {
    use std::io::{Read, Write};
    // The socket and the block length bound only this regression.
    let path = PathBuf::from("/tmp").join(format!("echode-rta03-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).expect("gate socket");
    let fault = format!("process-wait:{}", path.display());
    let mut host = spawn_plugin(Some(&fault), SessionLayout::stereo());
    let (mut gate, _) = listener
        .accept()
        .expect("the plugin connects at activation");
    gate.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("gate timeout");
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the first block");
    // SAFETY: this test is the only user of each endpoint, and uses the old
    // mapping's endpoint only to submit and poll while the new one is prepared.
    let mut old = unsafe { host.audio_handle() };
    old.submit(32, &planes, &[])
        .expect("the block the plugin holds");
    let mut receipt = [0];
    gate.read_exact(&mut receipt)
        .expect("the plugin is inside process");
    let queued = old
        .submit(32, &planes, &[(note(), 0)])
        .expect("a later block is published while the helper is busy");
    let mut layout = SessionLayout::stereo();
    layout.input_events = 64;
    host.prepare_mapping(layout)
        .expect("a larger mapping is prepared");
    // SAFETY: as above; the new mapping's endpoint only activates it.
    let mut new = unsafe { host.audio_handle() };
    new.activate_mapping()
        .expect("the new mapping is activated");
    gate.write_all(&[1]).expect("release the held block");
    gate.read_exact(&mut receipt)
        .expect("the held block returns");
    let started = Instant::now();
    loop {
        match old.poll(queued, &mut outs(&mut output), &mut []) {
            Ok(BlockProgress::Ready { .. }) => break,
            Ok(_) => {}
            Err(error) => panic!("the helper failed: {error}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the helper adopted the new mapping and left the published block, with its note, unprocessed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_file(&path);
}

fn note() -> Event {
    Event::NoteOn(NoteEvent {
        time: 3,
        track: 1,
        target: Target::note(9, 60),
        velocity: 0.5,
    })
}

#[test]
fn a_synth_session_has_no_input_channels() {
    let mut host = spawn_layout(SessionLayout {
        max_frames: 64,
        input_channels: 0,
        output_channels: 4,
        input_events: 4,
        output_events: 4,
    });
    let mut first = vec![1.0f32; 16];
    let mut second = vec![1.0f32; 16];
    let mut third = vec![1.0f32; 16];
    let mut fourth = vec![1.0f32; 16];
    let mut planes: [&mut [f32]; 4] = [&mut first, &mut second, &mut third, &mut fourth];
    let event = note();
    let mut decoded = [event, event, event, event];
    let count = host
        .process_events(
            &[],
            &mut planes,
            &[event],
            &mut decoded,
            Duration::from_secs(2),
        )
        .expect("synth block");
    assert_eq!(count, 1);
    assert_eq!(decoded[0], event);
    assert!(
        [first, second, third, fourth]
            .iter()
            .all(|plane| plane.iter().all(|sample| *sample == 0.0))
    );
}

#[test]
fn a_midi_only_session_round_trips_events() {
    let mut host = spawn_layout(SessionLayout {
        max_frames: 32,
        input_channels: 0,
        output_channels: 0,
        input_events: 4,
        output_events: 4,
    });
    let event = note();
    let mut decoded = [event, event];
    let count = host
        .process_block(
            8,
            &[],
            &mut [],
            &[event],
            &mut decoded,
            Duration::from_secs(2),
        )
        .expect("midi block");
    assert_eq!(count, 1);
    assert_eq!(decoded[0], event);
}

fn refused_launch(plugin: &Path, plugin_id: Option<&str>, fault: Option<&str>) -> SessionError {
    match PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(plugin),
        plugin_id,
        fault,
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 0,
    }) {
        Ok(_) => panic!("a refused launch succeeded"),
        Err(error) => error,
    }
}

fn launch_cause(error: &SessionError) -> &LaunchFailure {
    let mut cause = error;
    while let SessionError::Launch { error, .. } = cause {
        cause = error;
    }
    let SessionError::Refused(Refusal::Launch(failure)) = cause else {
        panic!("launch refusal lost its kind: {error:?}");
    };
    failure
}

#[test]
fn launch_port_refusals_remain_plugin_failures() {
    let plugin = plugin_dylib();
    let error = refused_launch(&plugin, Some("dev.echode.test-noports"), None);
    let mut cause = &error;
    while let SessionError::Launch { error, .. } = cause {
        cause = error;
    }
    assert!(matches!(
        cause,
        SessionError::Refused(Refusal::Plugin(echode_sandbox::PluginFailure::NoAudioPorts))
    ));
    assert_eq!(cause.to_string(), "plugin does not declare audio ports");
}

#[test]
fn invalid_parameter_id_keeps_its_kind_and_rejects_the_entire_batch() {
    let plugin = plugin_dylib();
    let host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin),
        plugin_id: Some("dev.echode.test-params"),
        fault: None,
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 0,
    })
    .unwrap();
    let control = host.control_handle();
    let before = control.capture_state().unwrap();
    let error = control
        .set_parameters(&[
            echode_sandbox::ParamChange { id: 1, value: 0.25 },
            echode_sandbox::ParamChange {
                id: u32::MAX,
                value: 1.0,
            },
        ])
        .expect_err("the reserved CLAP id must be rejected");
    assert!(
        matches!(
            error,
            SessionError::Refused(Refusal::InvalidParameterId(u32::MAX))
        ),
        "invalid parameter id lost its kind: {error:?}"
    );
    assert_eq!(error.to_string(), "invalid parameter change");
    assert_eq!(control.capture_state().unwrap(), before);
    // Other ids still reach the plugin; unknown ids are not the sentinel.
    let values = control
        .set_parameters(&[
            echode_sandbox::ParamChange { id: 0, value: 1.0 },
            echode_sandbox::ParamChange {
                id: u32::MAX - 1,
                value: 1.0,
            },
        ])
        .unwrap();
    assert!(values.is_empty());
    assert_eq!(control.capture_state().unwrap(), before);
    let values = control
        .set_parameters(&[echode_sandbox::ParamChange { id: 1, value: 0.5 }])
        .unwrap();
    assert_eq!(values[0].value, 0.5);
}

// Exercise the real launch handshake, not an in-process loader or a text parser.
#[test]
fn launch_failures_keep_their_kinds_across_the_helper() {
    let plugin = plugin_dylib();
    for (plugin_id, fault, expected, diagnostic) in [
        (
            Some("dev.echode.not-present"),
            None,
            LaunchFailureKind::NoPlugin,
            format!("{} does not contain a plugin", plugin.display()),
        ),
        (
            None,
            Some("generate"),
            LaunchFailureKind::PluginNotFound,
            "Specified plugin was not found".to_string(),
        ),
        (
            None,
            Some("activate"),
            LaunchFailureKind::ActivationFailed,
            "Unable to activate".to_string(),
        ),
    ] {
        let error = refused_launch(&plugin, plugin_id, fault);
        let failure = launch_cause(&error);
        assert_eq!(failure.kind, expected);
        assert_eq!(failure.to_string(), diagnostic);
    }
}

#[test]
fn launch_entry_failures_keep_errno_and_the_native_loader_diagnostic() {
    // A regular file cannot have a child: no fixture or installed plugin is read.
    let path = plugin_dylib().join("not-a-plugin");
    let error = refused_launch(&path, None, None);
    let failure = launch_cause(&error);
    assert_eq!(
        failure.kind,
        LaunchFailureKind::EntryIo {
            raw_os_error: Some(libc::ENOTDIR)
        }
    );
    assert_eq!(
        failure.to_string(),
        std::io::Error::from_raw_os_error(libc::ENOTDIR).to_string()
    );

    // A text manifest exists but is not a dynamic library. Only the helper loads it.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let error = refused_launch(&path, None, None);
    let failure = launch_cause(&error);
    assert_eq!(failure.kind, LaunchFailureKind::Library);
    assert!(
        failure
            .diagnostic
            .starts_with("Failed to load plugin descriptor library:")
    );
    assert!(failure.diagnostic.contains(&path.display().to_string()));
}

#[test]
fn dirty_and_restored_state_cross_the_socket() {
    let mut host = spawn(None, Some("dirty"));
    let started = Instant::now();
    loop {
        if matches!(
            host.poll_notice().expect("notice"),
            Some(PluginNotice::Dirty)
        ) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "dirty notice did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = host
        .control_handle()
        .restore_state(b"abc")
        .expect_err("restore without a plugin");
    assert!(error.to_string().contains("no plugin"), "{error}");
}

#[test]
fn exit_is_observed_without_waiting_out_the_deadline() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), Some("crash"));
    let input = stereo(32, 0.0);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    let started = Instant::now();
    let error = host
        .process(&planes, &mut outs(&mut output), CRASH_DEADLINE)
        .expect_err("crash");
    assert!(
        started.elapsed() < CRASH_DEADLINE / 2,
        "exit was observed only after {:?}",
        started.elapsed()
    );
    assert!(matches!(error, SessionError::HostExited { .. }));
    host.restart().expect("reattach");
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("block after restart");
    assert!((output[0][0] - input[0][0] * 2.0).abs() < 1.0e-6);
}

#[test]
fn remap_switches_to_a_mono_output() {
    let mut host = spawn_layout(SessionLayout::stereo());
    host.remap(SessionLayout {
        max_frames: 32,
        input_channels: 0,
        output_channels: 1,
        input_events: 2,
        output_events: 2,
    })
    .expect("remap");
    let mut output = vec![1.0f32; 8];
    let mut planes: [&mut [f32]; 1] = [&mut output];
    let event = note();
    let mut decoded = [event, event];
    let count = host
        .process_events(
            &[],
            &mut planes,
            &[event],
            &mut decoded,
            Duration::from_secs(2),
        )
        .expect("remapped block");
    assert_eq!(count, 1);
    assert_eq!(decoded[0], event);
    assert!(output.iter().all(|sample| *sample == 0.0));
}

fn spawn_fan(plugin: Option<&Path>, fault: Option<&str>, fan: &Arc<FanIn>) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin,
        fault,
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: Some(Arc::clone(fan)),
        sample_rate: 0,
        plugin_id: None,
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
}

fn submit_stereo(host: &mut PluginHost, frames: &[Vec<f32>; 2]) -> u64 {
    host.submit(frames[0].len(), &[&frames[0], &frames[1]], &[])
        .unwrap_or_else(|error| panic!("submit: {error}"))
}

fn poll_stereo(
    host: &mut PluginHost,
    generation: u64,
    frames: usize,
) -> (BlockProgress, [Vec<f32>; 2]) {
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    let progress = host
        .poll(generation, &mut [&mut left[..], &mut right[..]], &mut [])
        .unwrap_or_else(|error| panic!("poll: {error}"));
    (progress, [left, right])
}

fn await_ready(
    fan: &FanIn,
    host: &mut PluginHost,
    generation: u64,
    frames: usize,
) -> [Vec<f32>; 2] {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = fan.load();
    loop {
        let (progress, output) = poll_stereo(host, generation, frames);
        if matches!(progress, BlockProgress::Ready { .. }) {
            return output;
        }
        seen = fan
            .wait_until(seen, deadline)
            .unwrap_or_else(|error| panic!("fan-in: {error}"));
    }
}

#[test]
fn two_helpers_finish_in_about_one_round_trip() {
    let fan = Arc::new(FanIn::create().expect("fan-in"));
    let mut first = spawn_fan(None, None, &fan);
    let mut second = spawn_fan(None, None, &fan);
    let input = stereo(64, 0.2);
    for _ in 0..10 {
        let generation = submit_stereo(&mut first, &input);
        let _ = await_ready(&fan, &mut first, generation, 64);
    }
    let mut single = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        let generation = submit_stereo(&mut first, &input);
        let output = await_ready(&fan, &mut first, generation, 64);
        assert_eq!(output[0], input[0]);
        single.push(started.elapsed());
    }
    single.sort_unstable();
    let one = single[single.len() / 2];
    let mut parallel = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        let left = submit_stereo(&mut first, &input);
        let right = submit_stereo(&mut second, &input);
        let deadline = started + Duration::from_secs(2);
        let mut seen = fan.load();
        let mut left_done = false;
        let mut right_done = false;
        loop {
            if !left_done {
                let (progress, output) = poll_stereo(&mut first, left, 64);
                if matches!(progress, BlockProgress::Ready { .. }) {
                    assert_eq!(output[0], input[0]);
                    left_done = true;
                }
            }
            if !right_done {
                let (progress, output) = poll_stereo(&mut second, right, 64);
                if matches!(progress, BlockProgress::Ready { .. }) {
                    assert_eq!(output[0], input[0]);
                    right_done = true;
                }
            }
            if left_done && right_done {
                break;
            }
            seen = fan
                .wait_until(seen, deadline)
                .unwrap_or_else(|error| panic!("fan-in: {error}"));
        }
        parallel.push(started.elapsed());
    }
    parallel.sort_unstable();
    let both = parallel[parallel.len() / 2];
    if !cfg!(debug_assertions) {
        assert!(
            both < one + one / 2,
            "two helpers took {both:?}, one took {one:?}"
        );
    }
}

#[test]
fn a_fast_helper_is_collected_while_the_other_is_still_running() {
    let fan = Arc::new(FanIn::create().expect("fan-in"));
    let plugin = plugin_dylib();
    let mut slow = spawn_fan(Some(&plugin), Some("hang"), &fan);
    let mut fast = spawn_fan(None, None, &fan);
    let input = stereo(32, 0.1);
    let started = Instant::now();
    let hung = submit_stereo(&mut slow, &input);
    let copied = submit_stereo(&mut fast, &input);
    let deadline = started + Duration::from_millis(80);
    let mut seen = fan.load();
    let mut unlocked = None;
    loop {
        if unlocked.is_none() {
            let (progress, output) = poll_stereo(&mut fast, copied, 32);
            if matches!(progress, BlockProgress::Ready { .. }) {
                assert_eq!(output[0], input[0]);
                unlocked = Some(started.elapsed());
            }
        }
        if let Some(unlocked) = unlocked {
            let (progress, _) = poll_stereo(&mut slow, hung, 32);
            assert!(matches!(progress, BlockProgress::Pending));
            assert!(
                unlocked < Duration::from_millis(20),
                "unlocked after {unlocked:?}"
            );
            slow.abandon(hung);
            break;
        }
        assert!(Instant::now() < deadline, "the fast helper did not finish");
        seen = fan
            .wait_until(seen, deadline)
            .unwrap_or_else(|error| panic!("fan-in: {error}"));
    }
}

#[test]
fn every_helper_shares_one_deadline() {
    let fan = Arc::new(FanIn::create().expect("fan-in"));
    let plugin = plugin_dylib();
    let mut first = spawn_fan(Some(&plugin), Some("hang"), &fan);
    let mut second = spawn_fan(Some(&plugin), Some("hang"), &fan);
    let input = stereo(16, 0.0);
    let started = Instant::now();
    let left = submit_stereo(&mut first, &input);
    let right = submit_stereo(&mut second, &input);
    let deadline = started + Duration::from_millis(40);
    let mut seen = fan.load();
    loop {
        let (first_progress, _) = poll_stereo(&mut first, left, 16);
        let (second_progress, _) = poll_stereo(&mut second, right, 16);
        if Instant::now() >= deadline {
            assert!(matches!(first_progress, BlockProgress::Pending));
            assert!(matches!(second_progress, BlockProgress::Pending));
            first.abandon(left);
            second.abandon(right);
            break;
        }
        let _ = fan.wait_until(seen, deadline);
        seen = fan.load();
    }
    assert!(
        started.elapsed() < Duration::from_millis(70),
        "waits stacked to {:?}",
        started.elapsed()
    );
}

#[test]
fn two_stages_in_one_helper_are_one_round_trip() {
    let plugin = plugin_dylib();
    let mut host = spawn(Some(&plugin), None);
    host.set_chain(2).expect("chain");
    let input = stereo(32, 0.25);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("chained block");
    assert!((output[0][0] - input[0][0] * 4.0).abs() < 1.0e-5);

    let fan = Arc::new(FanIn::create().expect("fan-in"));
    let mut routed = spawn_fan(Some(&plugin), None, &fan);
    routed.set_chain(2).expect("chain");
    let kept = note();
    let dropped = Event::NoteOff(NoteEvent {
        time: 1,
        track: 2,
        target: Target::note(3, 62),
        velocity: 0.2,
    });
    let generation = routed
        .submit(
            8,
            &[&input[0][..8], &input[1][..8]],
            &[(kept, 0), (dropped, 1), (dropped, 5)],
        )
        .expect("submit");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = fan.load();
    let mut slot = [
        Delivered {
            event: kept,
            device: 0,
        },
        Delivered {
            event: kept,
            device: 0,
        },
        Delivered {
            event: kept,
            device: 0,
        },
    ];
    loop {
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let progress = routed
            .poll(generation, &mut [&mut left[..], &mut right[..]], &mut slot)
            .expect("poll");
        if let BlockProgress::Ready { events } = progress {
            assert_eq!(events, 2);
            assert_eq!(slot[0].device, 0);
            assert_eq!(slot[1].device, 1);
            assert_eq!(slot[0].event, kept);
            break;
        }
        seen = fan.wait_until(seen, deadline).expect("fan-in");
    }
}

#[test]
fn prepared_mapping_keeps_old_audio_live_until_publication_and_can_be_cancelled_under_rtsan() {
    let path = plugin_dylib();
    let mut host = spawn(Some(&path), None);
    // SAFETY: fixture endpoints are processed sequentially, with the host retained.
    let mut old = unsafe { host.audio_handle() };
    let original = host.layout();
    let mut next = original;
    next.input_events += 37; // Fixture growth, not an event limit.
    host.prepare_mapping(next).unwrap();
    assert!(host.mapping_pending());
    let frames = 64; // Fixture buffers only.
    let input = vec![vec![0.125; frames], vec![-0.25; frames]];
    let mut output = vec![vec![0.0; frames]; 2];
    let exchange = |audio: &mut echode_sandbox::PluginAudio, output: &mut [Vec<f32>]| {
        let generation = audio.submit_planes(frames, &input, &[]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match audio
                .poll_planes(generation, output, frames, &mut [])
                .unwrap()
            {
                BlockProgress::Ready { .. } => break,
                BlockProgress::Pending if Instant::now() < deadline => std::thread::yield_now(),
                other => panic!("mapping exchange: {other:?}"),
            }
        }
        assert!(output[0].iter().all(|sample| *sample == 0.25));
        assert!(output[1].iter().all(|sample| *sample == -0.5));
    };
    for _ in 0..4 {
        exchange(&mut old, &mut output);
    }
    assert!(
        host.mapping_pending(),
        "old requests activated unpublished storage"
    );
    host.cancel_mapping().unwrap();
    assert_eq!(host.layout(), original);
    exchange(&mut old, &mut output);
    host.prepare_mapping(next).unwrap();
    // SAFETY: fixture endpoints are processed sequentially, with the host retained.
    let mut replacement = unsafe { host.audio_handle() };
    exchange(&mut replacement, &mut output);
    assert!(!host.mapping_pending());
    host.prepare_mapping(original).unwrap();
    // SAFETY: fixture endpoints are processed sequentially, with the host retained.
    let mut shrunk = unsafe { host.audio_handle() };
    exchange(&mut replacement, &mut output);
    exchange(&mut shrunk, &mut output);
    assert!(!host.mapping_pending());
}

#[test]
fn an_endpoint_observes_a_crash_after_mapping_replacement_under_rtsan() {
    let path = plugin_dylib();
    let mut host = spawn(Some(&path), Some("crash"));
    let mut next = host.layout();
    next.input_events += 1;
    host.prepare_mapping(next).unwrap();
    // SAFETY: fixture endpoints are processed sequentially, with the host retained.
    let mut audio = unsafe { host.audio_handle() };
    // Fixture buffers only; the deadline detects a missing exit notification.
    let input = vec![vec![0.0; 16]; 2];
    let mut output = input.clone();
    let generation = audio.submit_planes(16, &input, &[]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match audio.poll_planes(generation, &mut output, 16, &mut []) {
            Err(SessionError::HelperDown) => break,
            Ok(BlockProgress::Pending) if Instant::now() < deadline => std::thread::yield_now(),
            other => panic!("replacement mapping lost exit notification: {other:?}"),
        }
    }
}

/// R8 (ADR 0318). A plugin state longer than one control frame's body used to
/// stop the helper: its save reply could not be framed, the control loop
/// ended, and the plugin fell silent. The state now crosses in parts both ways
/// and the plugin keeps sounding.
#[test]
fn a_state_longer_than_one_frame_saves_and_restores_in_parts() {
    // The test plugin's `large-state` length: one frame's largest body
    // (64 MiB less type and id) plus 1 MiB. It sizes this fixture only.
    const LARGE_STATE: usize = 64 * 1024 * 1024 - 12 + 1024 * 1024;
    let mut host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin_dylib()),
        fault: Some("large-state"),
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some("dev.echode.test-state"),
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"));
    // Fixture buffers only; the state plugin writes its level to every frame.
    let input = stereo(32, 0.0);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("initial block");
    assert!((output[0][0] - 0.25).abs() < 1.0e-4, "{}", output[0][0]);

    let control = host.control_handle();
    let saved = control
        .capture_state()
        .unwrap_or_else(|error| panic!("a state past one frame must save: {error:?}"));
    assert_eq!(saved.len(), LARGE_STATE);
    assert!(saved.iter().all(|byte| *byte == 64), "saved bytes changed");
    drop(saved);
    control
        .restore_state(&vec![128; LARGE_STATE])
        .unwrap_or_else(|error| panic!("a state past one frame must restore: {error:?}"));
    let saved = control.capture_state().expect("save after the restore");
    assert_eq!(saved.len(), LARGE_STATE);
    assert!(
        saved.iter().all(|byte| *byte == 128),
        "restored bytes changed"
    );
    drop(saved);
    host.process(&planes, &mut outs(&mut output), Duration::from_secs(2))
        .expect("the helper still processes after both transfers");
    assert!((output[0][0] - 0.5).abs() < 1.0e-4, "{}", output[0][0]);
}

/// R9 (ADR 0319): a background save's reply and a restore, each larger than
/// both socket buffers, used to stop both ends in their sends for good. Each
/// end now reads what the other queued while it waits to send.
#[test]
fn a_large_restore_crosses_a_large_background_save_without_stalling() {
    // The test plugin's `large-state` length, as above. It sizes this fixture only.
    const LARGE_STATE: usize = 64 * 1024 * 1024 - 12 + 1024 * 1024;
    let host = PluginHost::spawn(SpawnOptions {
        cancel: None,
        host_bin: &host_bin(),
        plugin: Some(&plugin_dylib()),
        fault: Some("large-state"),
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: None,
        sample_rate: 48_000,
        plugin_id: Some("dev.echode.test-state"),
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"));
    let mut control = host.control_handle();
    // A loaded large state marks the plugin dirty.
    control
        .restore_state(&vec![64; LARGE_STATE])
        .unwrap_or_else(|error| panic!("first restore: {error:?}"));
    // The helper sends the dirty notice before it reads the next request, so
    // the notice is queued here once this reply has arrived.
    let _ = control.parameters();
    // Takes the notice and asks for a background save. The helper starts
    // sending a state larger than both socket buffers.
    assert!(
        control.poll_state_save().is_none(),
        "the save cannot be back yet"
    );

    let restorer = control.clone();
    let (done, finished) = std::sync::mpsc::channel();
    let restoring = std::thread::spawn(move || {
        let _ = done.send(restorer.restore_state(&vec![128; LARGE_STATE]));
    });
    let restored = finished.recv_timeout(Duration::from_secs(30));
    if restored.is_err() {
        // Stopping the helper fails the stalled send, so the thread can end.
        drop(host);
        let _ = restoring.join();
        panic!("the restore stalled behind the helper's background save reply");
    }
    restoring.join().expect("restore thread");
    restored
        .expect("restore result")
        .unwrap_or_else(|error| panic!("restore while a save is sent: {error:?}"));

    let deadline = Instant::now() + Duration::from_secs(10);
    let saved = loop {
        if let Some(bytes) = control.poll_state_save() {
            break bytes;
        }
        assert!(
            Instant::now() < deadline,
            "the background save never arrived"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(saved.len(), LARGE_STATE);
    assert!(
        saved.iter().all(|byte| *byte == 64),
        "the save ran before the restore"
    );
    drop(saved);
    assert_eq!(
        control
            .applied_state()
            .map(|state| state.bytes().len() == LARGE_STATE && state.bytes()[0] == 128),
        Some(true),
        "the older save must not replace the restored bytes"
    );
}

/// R9 (ADR 0319): a `CancelRemap` waits for the processing thread to switch,
/// and a hung plugin never lets it, so the helper stops reading. A restore
/// past the socket buffers used to wait in its send for good, holding the
/// control socket. It now ends at its deadline and later requests fail at once.
#[test]
fn a_restore_behind_a_cancel_held_by_a_hung_plugin_ends_at_its_deadline() {
    // Twice both 1 MiB socket buffers. It sizes this fixture only.
    const PAST_BUFFERS: usize = 4 * 1024 * 1024;
    let mut host = spawn(Some(&plugin_dylib()), Some("hang"));
    let input = stereo(32, 0.2);
    let mut output = [vec![0.0; 32], vec![0.0; 32]];
    let planes: [&[f32]; 2] = [&input[0], &input[1]];
    assert!(matches!(
        host.process(&planes, &mut outs(&mut output), Duration::from_millis(100)),
        Err(SessionError::Timeout)
    ));
    let mut next = host.layout();
    next.input_events += 1; // Fixture growth, not an event limit.
    host.prepare_mapping(next)
        .expect("the main thread still prepares");
    assert!(
        matches!(host.cancel_mapping(), Err(SessionError::Timeout)),
        "the hung processing thread never takes the cancel"
    );

    let control = host.control_handle();
    let restorer = control.clone();
    let (done, finished) = std::sync::mpsc::channel();
    let start = Instant::now();
    let restoring = std::thread::spawn(move || {
        let _ = done.send(restorer.restore_state(&vec![1; PAST_BUFFERS]));
    });
    let Ok(result) = finished.recv_timeout(Duration::from_secs(10)) else {
        // Stopping the helper fails the stalled send, so the thread can end.
        drop(host);
        let _ = restoring.join();
        panic!("the restore waited in its send for a helper that stopped reading");
    };
    restoring.join().expect("restore thread");
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "{:?}",
        start.elapsed()
    );
    let send_cause = |error: SessionError| match error {
        SessionError::Io(error) => (
            error.kind(),
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<ControlSendError>())
                .copied(),
        ),
        other => panic!("a send failure stays a typed I/O cause: {other:?}"),
    };
    assert!(matches!(
        send_cause(result.expect_err("the cut-off restore fails")),
        (
            std::io::ErrorKind::TimedOut,
            Some(ControlSendError::Abandoned { .. })
        )
    ));
    let start = Instant::now();
    assert_eq!(
        send_cause(control.parameters().expect_err("nothing more is sent")),
        (
            std::io::ErrorKind::BrokenPipe,
            Some(ControlSendError::Closed)
        )
    );
    assert!(start.elapsed() < Duration::from_millis(500));
}

/// Newest edit among the test plugin's sources, so a change to any module rebuilds it.
fn newest_source(dir: &Path) -> std::io::Result<std::time::SystemTime> {
    let mut newest = std::time::SystemTime::UNIX_EPOCH;
    for entry in std::fs::read_dir(dir)? {
        newest = newest.max(entry?.metadata()?.modified()?);
    }
    Ok(newest)
}
