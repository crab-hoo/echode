//! Phase 0 device measurement, excluded from default product builds.

use super::*;
use crate::graph::{BLOCK_FRAMES, Engine};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const PROCESS_MUTE: u32 = u32::from_be_bytes(*b"appm");
const TARGET_RATE: f64 = 48_000.0;
const WARMUP_CALLBACKS: u32 = 200;
// Liveness guard for this measurement, not a realtime processing budget.
const CALLBACK_STALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementEnd {
    Completed,
    Interrupted,
    CallbacksStopped,
    DefaultOutputChanged { expected: u32, actual: u32 },
    DeviceUnavailable(i32),
    ClockDiscontinuity,
}

#[derive(Clone, Copy)]
struct Observation {
    elapsed: Duration,
    wall_elapsed: Option<Duration>,
    callbacks: u32,
    output: Result<u32, i32>,
    interrupted: bool,
}

/// The same control loop drives the real device and deterministic fault tests.
/// It never runs on the audio thread.
fn measure_progress(
    duration: Duration,
    device: u32,
    mut observe: impl FnMut() -> Observation,
    mut wait: impl FnMut(),
    mut report: impl FnMut(),
) -> MeasurementEnd {
    let mut previous = 0;
    let mut last_progress = Duration::ZERO;
    let mut next_report = Duration::from_secs(30);
    loop {
        let observation = observe();
        if observation.interrupted {
            return MeasurementEnd::Interrupted;
        }
        // On macOS the monotonic clock can pause during sleep. Reject a run
        // across suspend or a large wall-clock adjustment even if HAL resumes
        // before the next poll and no missing callback is directly observed.
        if observation
            .wall_elapsed
            .is_none_or(|wall| wall.abs_diff(observation.elapsed) >= CALLBACK_STALL_TIMEOUT)
        {
            return MeasurementEnd::ClockDiscontinuity;
        }
        match observation.output {
            Ok(actual) if actual != device => {
                return MeasurementEnd::DefaultOutputChanged {
                    expected: device,
                    actual,
                };
            }
            Err(status) => return MeasurementEnd::DeviceUnavailable(status),
            _ => {}
        }
        if observation.callbacks != previous {
            previous = observation.callbacks;
            last_progress = observation.elapsed;
        }
        if observation.elapsed.saturating_sub(last_progress) >= CALLBACK_STALL_TIMEOUT {
            return MeasurementEnd::CallbacksStopped;
        }
        if observation.elapsed >= duration {
            return MeasurementEnd::Completed;
        }
        if observation.elapsed >= next_report {
            report();
            next_report = observation.elapsed + Duration::from_secs(30);
        }
        wait();
    }
}

struct Restore {
    device: u32,
    buffer_frames: u32,
    mute: Option<u32>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = set_u32(self.device, BUFFER_FRAMES, SCOPE_OUTPUT, self.buffer_frames);
        let _ = set_u32(self.device, BUFFER_FRAMES, SCOPE_GLOBAL, self.buffer_frames);
        if let Some(mute) = self.mute {
            let _ = set_u32(self.device, PROCESS_MUTE, SCOPE_OUTPUT, mute);
            let _ = set_u32(self.device, PROCESS_MUTE, SCOPE_GLOBAL, mute);
        }
    }
}

pub struct Report {
    pub requested_seconds: u64,
    pub measured_for: Duration,
    pub sample_rate: f64,
    pub buffer_frames: u32,
    pub callbacks: u32,
    pub late: u32,
    pub gaps: u32,
    pub warmup_late: u32,
    pub max_ns: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub budget_ns: u64,
    pub workgroup_failures: u64,
    pub policy_failures: u64,
    pub workgroup_parallel: i32,
    pub workers: usize,
    pub outcome: MeasurementEnd,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.outcome == MeasurementEnd::Completed
            && self.measured_for >= Duration::from_secs(self.requested_seconds)
            && self.late == 0
            && self.gaps == 0
            && self.workgroup_failures == 0
            && self.policy_failures == 0
            && self.buffer_frames == BLOCK_FRAMES as u32
            && self.callbacks > WARMUP_CALLBACKS
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "duration: requested {} s, measured {:.3} s",
            self.requested_seconds,
            self.measured_for.as_secs_f64()
        )?;
        writeln!(
            f,
            "device: {:.0} Hz, {} frames (budget {:.3} ms)",
            self.sample_rate,
            self.buffer_frames,
            self.budget_ns as f64 / 1_000_000.0
        )?;
        writeln!(
            f,
            "callbacks: {} measured after {WARMUP_CALLBACKS} warmup",
            self.callbacks.saturating_sub(WARMUP_CALLBACKS)
        )?;
        writeln!(
            f,
            "cpu p50 {:.3}  p99 {:.3}  max {:.3} ms",
            self.p50_ns as f64 / 1_000_000.0,
            self.p99_ns as f64 / 1_000_000.0,
            self.max_ns as f64 / 1_000_000.0
        )?;
        writeln!(
            f,
            "late: {}   host-time gaps: {}   warmup late: {}",
            self.late, self.gaps, self.warmup_late
        )?;
        writeln!(f, "workgroup join failures: {}", self.workgroup_failures)?;
        writeln!(f, "time-constraint failures: {}", self.policy_failures)?;
        writeln!(f, "workgroup parallel threads: {}", self.workgroup_parallel)?;
        writeln!(f, "workers: {}", self.workers)?;
        writeln!(f, "measurement: {:?}", self.outcome)?;
        writeln!(f, "result: {}", if self.passed() { "pass" } else { "fail" })?;
        Ok(())
    }
}

struct Session {
    engine: Arc<Engine>,
    samples: std::cell::UnsafeCell<Box<[u32]>>,
    sample_count: AtomicU32,
    late: AtomicU32,
    gaps: AtomicU32,
    warmup_late: AtomicU32,
    max_ns: AtomicU64,
    previous_host: AtomicU64,
    budget_ticks: u64,
    numer: u64,
    denom: u64,
}

// SAFETY: the audio thread is the only writer, and the report is read after
// `AudioDeviceStop` has returned.
unsafe impl Sync for Session {}

impl Session {
    fn record(&self, elapsed_ticks: u64, host_time: u64) {
        let nanos = elapsed_ticks.saturating_mul(self.numer) / self.denom;
        let index = self.sample_count.fetch_add(1, Ordering::Relaxed);
        // SAFETY: only this thread writes, and `index` is the slot it owns.
        let samples = unsafe { &mut *self.samples.get() };
        if (index as usize) < samples.len() {
            samples[index as usize] = u32::try_from(nanos).unwrap_or(u32::MAX);
        }
        self.max_ns.fetch_max(nanos, Ordering::Relaxed);
        if index < WARMUP_CALLBACKS {
            if elapsed_ticks > self.budget_ticks {
                self.warmup_late.fetch_add(1, Ordering::Relaxed);
            }
            self.previous_host.store(host_time, Ordering::Relaxed);
            return;
        }
        if elapsed_ticks > self.budget_ticks {
            self.late.fetch_add(1, Ordering::Relaxed);
        }
        let previous = self.previous_host.swap(host_time, Ordering::Relaxed);
        if previous != 0
            && host_time.wrapping_sub(previous) > self.budget_ticks.saturating_mul(3) / 2
        {
            self.gaps.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// `workers` of `None` uses `os_workgroup_max_parallel_threads`, minus the IO thread.
pub fn run(seconds: u64, workers: Option<usize>) -> Result<Report, String> {
    run_measurement(seconds, workers, true)
}

/// Brief hardware checks use the current device rate without reconfiguring it.
pub fn run_at_device_rate(seconds: u64, workers: Option<usize>) -> Result<Report, String> {
    run_measurement(seconds, workers, false)
}

fn measurement_geometry(
    sample_rate: f64,
    seconds: u64,
    require_reference_rate: bool,
) -> Result<(u64, usize), String> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("invalid output sample rate".to_string());
    }
    if require_reference_rate && (sample_rate - TARGET_RATE).abs() > 1.0 {
        return Err(format!(
            "output device is at {sample_rate} Hz; this spike requires {TARGET_RATE}"
        ));
    }
    let budget_ns = (BLOCK_FRAMES as f64 * 1_000_000_000.0 / sample_rate) as u64;
    if budget_ns == 0 {
        return Err("output period cannot be represented".to_string());
    }
    // Storage covers this measurement, including control-loop overshoot and
    // warmup. It is not a project limit, and scales with the actual IO rate.
    let callbacks_per_second = (sample_rate / BLOCK_FRAMES as f64).ceil() as u128;
    let capacity = (u128::from(seconds) + 1)
        .checked_mul(callbacks_per_second)
        .and_then(|count| count.checked_add(u128::from(WARMUP_CALLBACKS)))
        .and_then(|count| usize::try_from(count).ok())
        .ok_or("measurement duration cannot be represented")?;
    Ok((budget_ns, capacity))
}

fn run_measurement(
    seconds: u64,
    workers: Option<usize>,
    require_reference_rate: bool,
) -> Result<Report, String> {
    let device = default_output().map_err(|status| format!("default output device: {status}"))?;
    let sample_rate =
        get_f64(device, SAMPLE_RATE).map_err(|status| format!("sample rate: {status}"))?;
    let (budget_ns, capacity) = measurement_geometry(sample_rate, seconds, require_reference_rate)?;
    let original_buffer =
        get_u32(device, BUFFER_FRAMES).map_err(|status| format!("buffer size: {status}"))?;
    let original_mute = get_u32(device, PROCESS_MUTE).ok();
    set_u32(device, BUFFER_FRAMES, SCOPE_GLOBAL, BLOCK_FRAMES as u32)
        .or_else(|_| set_u32(device, BUFFER_FRAMES, SCOPE_OUTPUT, BLOCK_FRAMES as u32))
        .map_err(|status| format!("could not set buffer to {BLOCK_FRAMES} frames: {status}"))?;
    let _restore = Restore {
        device,
        buffer_frames: original_buffer,
        mute: original_mute,
    };
    let actual_buffer =
        get_u32(device, BUFFER_FRAMES).map_err(|status| format!("read buffer: {status}"))?;
    if actual_buffer != BLOCK_FRAMES as u32 {
        return Err(format!(
            "device kept a buffer of {actual_buffer} frames after requesting {BLOCK_FRAMES}"
        ));
    }
    let _ = set_u32(device, PROCESS_MUTE, SCOPE_GLOBAL, 1);
    let _ = set_u32(device, PROCESS_MUTE, SCOPE_OUTPUT, 1);

    let mut timebase = Timebase { numer: 0, denom: 1 };
    // SAFETY: `timebase` is a valid out-parameter.
    unsafe { mach_timebase_info(&mut timebase) };
    let budget_ticks = (u128::from(budget_ns) * u128::from(timebase.denom)
        / u128::from(timebase.numer.max(1))) as u64;

    let workgroup =
        get_pointer(device, WORKGROUP).map_err(|status| format!("workgroup: {status}"))?;
    if workgroup.is_null() {
        return Err("device did not return an IO workgroup".to_string());
    }

    let workgroup_parallel = crate::graph::workgroup_parallel_threads(workgroup);
    let worker_count = match workers {
        Some(count) => count,
        None => {
            if workgroup_parallel <= 1 {
                unsafe { os_release(workgroup) };
                return Err(format!(
                    "os_workgroup_max_parallel_threads returned {workgroup_parallel}"
                ));
            }
            (workgroup_parallel as usize) - 1
        }
    };
    let period_ticks = u32::try_from(budget_ticks).unwrap_or(u32::MAX).max(1);
    let engine = Arc::new(Engine::new(worker_count));
    let workers = engine.spawn_workers(workgroup, period_ticks);
    let session = Box::new(Session {
        engine,
        samples: std::cell::UnsafeCell::new(vec![0u32; capacity].into_boxed_slice()),
        sample_count: AtomicU32::new(0),
        late: AtomicU32::new(0),
        gaps: AtomicU32::new(0),
        warmup_late: AtomicU32::new(0),
        max_ns: AtomicU64::new(0),
        previous_host: AtomicU64::new(0),
        budget_ticks,
        numer: u64::from(timebase.numer.max(1)),
        denom: u64::from(timebase.denom.max(1)),
    });

    let mut proc_id: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        AudioDeviceCreateIOProcID(
            device,
            io_proc,
            std::ptr::from_ref(&*session).cast_mut().cast(),
            &mut proc_id,
        )
    };
    if status != 0 {
        drop(workers);
        unsafe { os_release(workgroup) };
        return Err(format!("AudioDeviceCreateIOProcID: {status}"));
    }
    let status = unsafe { AudioDeviceStart(device, proc_id) };
    if status != 0 {
        unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
        drop(workers);
        unsafe { os_release(workgroup) };
        return Err(format!("AudioDeviceStart: {status}"));
    }

    let started = Instant::now();
    let wall_started = SystemTime::now();
    let outcome = measure_progress(
        Duration::from_secs(seconds),
        device,
        || Observation {
            elapsed: started.elapsed(),
            wall_elapsed: wall_started.elapsed().ok(),
            callbacks: session.sample_count.load(Ordering::Acquire),
            output: default_output(),
            interrupted: stop_requested(),
        },
        || thread::sleep(Duration::from_millis(200)),
        || {
            eprintln!(
                "callbacks={} late={} gaps={} max={:.3} ms",
                session.sample_count.load(Ordering::Relaxed),
                session.late.load(Ordering::Relaxed),
                session.gaps.load(Ordering::Relaxed),
                session.max_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
            );
        },
    );
    let measured_for = started.elapsed();

    unsafe { AudioDeviceStop(device, proc_id) };
    unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
    let failures = session.engine.workgroup_failures();
    let policy_failures = session.engine.policy_failures();
    drop(workers);
    unsafe { os_release(workgroup) };

    let count = session.sample_count.load(Ordering::Relaxed) as usize;
    let samples = unsafe { &*session.samples.get() };
    let measured = samples
        .get(WARMUP_CALLBACKS as usize..count.min(samples.len()))
        .unwrap_or(&[]);
    let mut sorted = measured.to_vec();
    sorted.sort_unstable();
    let percentile = |p: f64| -> u64 {
        if sorted.is_empty() {
            0
        } else {
            let index = ((sorted.len() - 1) as f64 * p).round() as usize;
            u64::from(sorted[index.min(sorted.len() - 1)])
        }
    };
    let late = session.late.load(Ordering::Relaxed);
    let gaps = session.gaps.load(Ordering::Relaxed);
    Ok(Report {
        requested_seconds: seconds,
        measured_for,
        sample_rate,
        buffer_frames: actual_buffer,
        callbacks: count as u32,
        late,
        gaps,
        warmup_late: session.warmup_late.load(Ordering::Relaxed),
        max_ns: session.max_ns.load(Ordering::Relaxed),
        p50_ns: percentile(0.50),
        p99_ns: percentile(0.99),
        budget_ns,
        workgroup_failures: failures,
        policy_failures,
        workgroup_parallel,
        workers: worker_count,
        outcome,
    })
}

unsafe extern "C" fn io_proc(
    _device: u32,
    _now: *const AudioTimeStamp,
    _input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    output_data: *mut AudioBufferList,
    output_time: *const AudioTimeStamp,
    client: *mut c_void,
) -> i32 {
    let session = unsafe { &*(client as *const Session) };
    // SAFETY: `mach_absolute_time` has no preconditions.
    let started = unsafe { mach_absolute_time() };
    let _energy = session.engine.render_block();
    silence(output_data);
    let elapsed = unsafe { mach_absolute_time() }.wrapping_sub(started);
    let host_time = if output_time.is_null() {
        0
    } else {
        unsafe { (*output_time).host_time }
    };
    session.record(elapsed, host_time);
    0
}

fn silence(list: *mut AudioBufferList) {
    if list.is_null() {
        return;
    }
    // SAFETY: CoreAudio passes a live variable-length buffer list for this callback.
    unsafe {
        let count = (*list).number_buffers as isize;
        let first = std::ptr::addr_of_mut!((*list).buffers).cast::<AudioBuffer>();
        for index in 0..count {
            let buffer = &*first.offset(index);
            if !buffer.data.is_null() && buffer.data_byte_size > 0 {
                std::ptr::write_bytes(buffer.data.cast::<u8>(), 0, buffer.data_byte_size as usize);
            }
        }
    }
}

fn stop_requested() -> bool {
    STOP.load(Ordering::Relaxed) != 0
}

static STOP: AtomicU32 = AtomicU32::new(0);

pub fn install_stop_handler() {
    unsafe {
        signal(2, handle_stop);
    }
}

unsafe extern "C" fn handle_stop(_signal: i32) {
    STOP.store(1, Ordering::Relaxed);
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
