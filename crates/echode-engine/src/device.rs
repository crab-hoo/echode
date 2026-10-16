//! CoreAudio output for the engine spike.
//!
//! The device buffer is forced to 64 frames and restored afterward. This
//! process is muted, and the callback writes silence, so the graph runs
//! against a real deadline without playing audio.

use crate::graph::{BLOCK_FRAMES, Engine};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const SYSTEM_OBJECT: u32 = 1;
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const SCOPE_OUTPUT: u32 = u32::from_be_bytes(*b"outp");
const ELEMENT_MAIN: u32 = 0;
const DEFAULT_OUTPUT: u32 = u32::from_be_bytes(*b"dOut");
const SAMPLE_RATE: u32 = u32::from_be_bytes(*b"nsrt");
const BUFFER_FRAMES: u32 = u32::from_be_bytes(*b"fsiz");
const WORKGROUP: u32 = u32::from_be_bytes(*b"oswg");
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

#[repr(C)]
struct PropAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct AudioTimeStamp {
    _sample_time: f64,
    host_time: u64,
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
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

fn default_output() -> Result<u32, i32> {
    get_u32(SYSTEM_OBJECT, DEFAULT_OUTPUT)
}

const ALL_DEVICES: u32 = u32::from_be_bytes(*b"dev#");
const STREAMS: u32 = u32::from_be_bytes(*b"stm#");

/// Output devices currently published by CoreAudio, plus the system default.
pub struct OutputInventory {
    pub default_device: u32,
    pub outputs: Vec<u32>,
}

/// The default output device's IO workgroup, retained until drop.
pub struct IoWorkgroup {
    object: *mut c_void,
    sample_rate: f64,
    frames: u32,
    period_ticks: u32,
    period_ns: u64,
}

impl IoWorkgroup {
    pub fn open() -> Result<Self, String> {
        let device =
            default_output().map_err(|status| format!("default output device: {status}"))?;
        let sample_rate =
            get_f64(device, SAMPLE_RATE).map_err(|status| format!("sample rate: {status}"))?;
        let frames =
            get_u32(device, BUFFER_FRAMES).map_err(|status| format!("buffer size: {status}"))?;
        let object =
            get_pointer(device, WORKGROUP).map_err(|status| format!("workgroup: {status}"))?;
        if object.is_null() {
            return Err("device did not return an IO workgroup".to_string());
        }
        let mut timebase = Timebase { numer: 0, denom: 1 };
        // SAFETY: `timebase` is a valid out-parameter.
        unsafe { mach_timebase_info(&mut timebase) };
        let numer = u128::from(timebase.numer.max(1));
        let denom = u128::from(timebase.denom.max(1));
        let rate = sample_rate.round().max(1.0) as u128;
        let ticks = (u128::from(frames) * 1_000_000_000 * denom) / (rate * numer);
        let period_ticks = u32::try_from(ticks).unwrap_or(u32::MAX).max(1);
        let period_ns = (u128::from(period_ticks) * numer / denom) as u64;
        Ok(Self {
            object,
            sample_rate,
            frames,
            period_ticks,
            period_ns,
        })
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.object
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn period_ticks(&self) -> u32 {
        self.period_ticks
    }

    pub fn period_ns(&self) -> u64 {
        self.period_ns
    }
}

impl Drop for IoWorkgroup {
    fn drop(&mut self) {
        if !self.object.is_null() {
            // SAFETY: CoreAudio returned this object retained, and nothing else releases it.
            unsafe { os_release(self.object) };
        }
    }
}

pub fn output_inventory() -> Result<OutputInventory, i32> {
    let default_device = default_output()?;
    let devices = get_u32s(SYSTEM_OBJECT, ALL_DEVICES, SCOPE_GLOBAL)?;
    let mut outputs = Vec::new();
    for device in devices {
        let streams = get_u32s(device, STREAMS, SCOPE_OUTPUT).unwrap_or_default();
        if !streams.is_empty() {
            outputs.push(device);
        }
    }
    Ok(OutputInventory {
        default_device,
        outputs,
    })
}

fn get_u32s(object: u32, selector: u32, scope: u32) -> Result<Vec<u32>, i32> {
    let mut size = 0u32;
    let addr = address(selector, scope);
    let status =
        unsafe { AudioObjectGetPropertyDataSize(object, &addr, 0, std::ptr::null(), &mut size) };
    if status != 0 {
        return Err(status);
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    // A runaway property must not be treated as a device list. This bound is
    // only a guard against a broken reply, not a limit on how many devices
    // CoreAudio may report.
    if size > 1_048_576 || !size.is_multiple_of(4) {
        return Err(-1);
    }
    let mut values = vec![0u32; size as usize / 4];
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            values.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    values.truncate(size as usize / 4);
    Ok(values)
}

fn address(selector: u32, scope: u32) -> PropAddress {
    PropAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    }
}

fn get_u32(object: u32, selector: u32) -> Result<u32, i32> {
    get_u32_scoped(object, selector, SCOPE_GLOBAL)
        .or_else(|_| get_u32_scoped(object, selector, SCOPE_OUTPUT))
}

fn get_u32_scoped(object: u32, selector: u32, scope: u32) -> Result<u32, i32> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let addr = address(selector, scope);
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    if status == 0 { Ok(value) } else { Err(status) }
}

fn set_u32(object: u32, selector: u32, scope: u32, value: u32) -> Result<(), i32> {
    let addr = address(selector, scope);
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            std::mem::size_of::<u32>() as u32,
            (&value as *const u32).cast(),
        )
    };
    if status == 0 { Ok(()) } else { Err(status) }
}

fn get_f64(object: u32, selector: u32) -> Result<f64, i32> {
    let mut value = 0f64;
    let mut size = std::mem::size_of::<f64>() as u32;
    let addr = address(selector, SCOPE_GLOBAL);
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    if status == 0 {
        Ok(value)
    } else {
        let addr = address(selector, SCOPE_OUTPUT);
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut value).cast(),
            )
        };
        if status == 0 { Ok(value) } else { Err(status) }
    }
}

fn get_pointer(object: u32, selector: u32) -> Result<*mut c_void, i32> {
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut c_void>() as u32;
    let addr = address(selector, SCOPE_GLOBAL);
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    };
    if status == 0 { Ok(value) } else { Err(status) }
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
mod tests {
    use super::*;

    fn observation(millis: u64, callbacks: u32) -> Observation {
        Observation {
            elapsed: Duration::from_millis(millis),
            wall_elapsed: Some(Duration::from_millis(millis)),
            callbacks,
            output: Ok(123),
            interrupted: false,
        }
    }

    fn outcome(observations: Vec<Observation>, duration: Duration) -> MeasurementEnd {
        let mut observations = observations.into_iter();
        measure_progress(
            duration,
            123,
            || observations.next().expect("measurement did not terminate"),
            || {},
            || {},
        )
    }

    fn report(outcome: MeasurementEnd, callbacks: u32) -> Report {
        Report {
            requested_seconds: 600,
            measured_for: Duration::from_secs(600),
            sample_rate: TARGET_RATE,
            buffer_frames: BLOCK_FRAMES as u32,
            callbacks,
            late: 0,
            gaps: 0,
            warmup_late: 0,
            max_ns: 300_000,
            p50_ns: 200_000,
            p99_ns: 250_000,
            budget_ns: 1_333_333,
            workgroup_failures: 0,
            policy_failures: 0,
            workgroup_parallel: 4,
            workers: 3,
            outcome,
        }
    }

    #[test]
    fn measurement_geometry_uses_the_device_rate_and_reserves_the_whole_run() {
        let (budget, slots) = measurement_geometry(96_000.0, 10, false).unwrap();
        assert_eq!(budget, 666_666);
        assert!(slots >= 15_000 + WARMUP_CALLBACKS as usize);
        let (budget, _) = measurement_geometry(44_100.0, 10, false).unwrap();
        assert_eq!(budget, 1_451_247);
        assert!(measurement_geometry(96_000.0, 10, true).is_err());
        assert!(measurement_geometry(48_000.0, 600, true).is_ok());
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(measurement_geometry(rate, 10, false).is_err());
        }
        assert!(measurement_geometry(96_000.0, u64::MAX, false).is_err());
    }

    #[test]
    fn contract_engine_measurement_requires_progress_and_completion() {
        // Synthetic callback counts and elapsed times exercise the real control
        // loop without hardware or wall-clock timing. They are not engine caps.
        let complete = outcome(
            (0..=5)
                .map(|step| observation(step * 200, step as u32 * 150))
                .collect(),
            Duration::from_secs(1),
        );
        assert_eq!(complete, MeasurementEnd::Completed);
        assert!(report(complete, 750).passed());

        let stalled = outcome(
            vec![
                observation(0, 0),
                observation(100, 77_386),
                observation(2000, 77_386),
                observation(2100, 77_386),
            ],
            Duration::from_secs(600),
        );
        assert_eq!(stalled, MeasurementEnd::CallbacksStopped);
        assert!(
            !report(stalled, 77_386).passed(),
            "nonempty timing samples hid a stopped callback"
        );

        let cold = outcome(
            vec![observation(0, 0), observation(2000, 0)],
            Duration::from_secs(600),
        );
        assert_eq!(cold, MeasurementEnd::CallbacksStopped);
        assert!(!report(cold, 0).passed());

        let interrupted = outcome(
            vec![
                observation(0, 0),
                Observation {
                    interrupted: true,
                    ..observation(400, 300)
                },
            ],
            Duration::from_secs(600),
        );
        assert_eq!(interrupted, MeasurementEnd::Interrupted);
        assert!(
            !report(interrupted, 300).passed(),
            "an interrupted run was treated as complete"
        );

        let changed = outcome(
            vec![
                observation(0, 0),
                Observation {
                    output: Ok(456),
                    ..observation(400, 300)
                },
            ],
            Duration::from_secs(600),
        );
        assert_eq!(
            changed,
            MeasurementEnd::DefaultOutputChanged {
                expected: 123,
                actual: 456
            }
        );
        assert!(!report(changed, 300).passed());
        let unavailable = outcome(
            vec![Observation {
                output: Err(-50),
                ..observation(400, 300)
            }],
            Duration::from_secs(600),
        );
        assert_eq!(unavailable, MeasurementEnd::DeviceUnavailable(-50));
        assert!(!report(unavailable, 300).passed());
        let resumed = outcome(
            vec![
                observation(0, 0),
                Observation {
                    wall_elapsed: Some(Duration::from_secs(30)),
                    ..observation(400, 300)
                },
            ],
            Duration::from_secs(600),
        );
        assert_eq!(resumed, MeasurementEnd::ClockDiscontinuity);
        assert!(!report(resumed, 300).passed());

        assert!(!report(MeasurementEnd::Completed, WARMUP_CALLBACKS).passed());
        let mut partial = report(MeasurementEnd::Completed, 77_386);
        partial.measured_for = Duration::from_secs(103);
        assert!(
            !partial.passed(),
            "partial elapsed time was treated as a complete measurement"
        );
        let mut invalid = report(MeasurementEnd::Completed, 750);
        invalid.late = 1;
        assert!(!invalid.passed());
        invalid.late = 0;
        invalid.gaps = 1;
        assert!(!invalid.passed());
    }
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const PropAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const PropAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
    fn AudioDeviceCreateIOProcID(
        device: u32,
        proc: unsafe extern "C" fn(
            u32,
            *const AudioTimeStamp,
            *const AudioBufferList,
            *const AudioTimeStamp,
            *mut AudioBufferList,
            *const AudioTimeStamp,
            *mut c_void,
        ) -> i32,
        client: *mut c_void,
        proc_id: *mut *mut c_void,
    ) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceStart(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, proc_id: *mut c_void) -> i32;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
    fn mach_absolute_time() -> u64;
    fn os_release(object: *mut c_void);
    fn signal(sig: i32, handler: unsafe extern "C" fn(i32)) -> *mut c_void;
}
