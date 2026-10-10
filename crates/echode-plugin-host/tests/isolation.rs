//! Cross-process hosting: audio actually crosses, a dead plugin does not kill us,
//! and the round trip is measured against the Phase 0 budget.

use echode_engine::IoWorkgroup;
use echode_plugin_host::measure_in_process;
use echode_realtime::{Event, NoteEvent, Target};
use echode_sandbox::{
    BlockProgress, Delivered, FanIn, PluginHost, PluginNotice, SessionError, SessionLayout,
    SpawnOptions, WorkgroupHandoff, WorkgroupMembership, copy_workgroup_port, join_workgroup,
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
    spawn_with(plugin, fault, None)
}

fn spawn_with(
    plugin: Option<&Path>,
    fault: Option<&str>,
    workgroup: Option<WorkgroupHandoff>,
) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        host_bin: &host_bin(),
        plugin,
        fault,
        workgroup,
        layout: SessionLayout::stereo(),
        fan_in: None,
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

fn spawn_layout(layout: SessionLayout) -> PluginHost {
    PluginHost::spawn(SpawnOptions {
        host_bin: &host_bin(),
        plugin: None,
        fault: None,
        workgroup: None,
        layout,
        fan_in: None,
    })
    .unwrap_or_else(|error| panic!("spawn plugin host: {error}"))
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
    host.restore_state(b"abc").expect("restore");
    assert_eq!(host.save_state().expect("save"), b"abc");
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
        host_bin: &host_bin(),
        plugin,
        fault,
        workgroup: None,
        layout: SessionLayout::stereo(),
        fan_in: Some(Arc::clone(fan)),
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
