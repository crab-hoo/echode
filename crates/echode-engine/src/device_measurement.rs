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

/// Why the measurement could not start (ADR 0294). It ends early only by
/// [`MeasurementEnd`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MeasurementError {
    Device(DeviceError),
    /// The output's nominal rate is not a positive finite number.
    InvalidSampleRate,
    /// The reference run needs [`TARGET_RATE`], and the device is at `rate`.
    RateMismatch {
        rate: f64,
    },
    /// One block at this rate is shorter than a nanosecond.
    PeriodTooShort,
    /// The run's sample storage cannot be addressed.
    DurationTooLong,
    /// The device refused the measurement's buffer size.
    SetBuffer(i32),
    /// The device accepted the request but kept this buffer size.
    BufferKept(u32),
    /// The workgroup allows this many threads, too few for automatic workers.
    ParallelThreads(i32),
    CreateIoProc(i32),
    StartIo(i32),
}

impl From<DeviceError> for MeasurementError {
    fn from(error: DeviceError) -> Self {
        Self::Device(error)
    }
}

impl std::fmt::Display for MeasurementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(error) => error.fmt(formatter),
            Self::InvalidSampleRate => formatter.write_str("invalid output sample rate"),
            Self::RateMismatch { rate } => write!(
                formatter,
                "output device is at {rate} Hz; this spike requires {TARGET_RATE}"
            ),
            Self::PeriodTooShort => formatter.write_str("output period cannot be represented"),
            Self::DurationTooLong => {
                formatter.write_str("measurement duration cannot be represented")
            }
            Self::SetBuffer(status) => write!(
                formatter,
                "could not set buffer to {BLOCK_FRAMES} frames: {status}"
            ),
            Self::BufferKept(frames) => write!(
                formatter,
                "device kept a buffer of {frames} frames after requesting {BLOCK_FRAMES}"
            ),
            Self::ParallelThreads(count) => {
                write!(
                    formatter,
                    "os_workgroup_max_parallel_threads returned {count}"
                )
            }
            Self::CreateIoProc(status) => write!(formatter, "AudioDeviceCreateIOProcID: {status}"),
            Self::StartIo(status) => write!(formatter, "AudioDeviceStart: {status}"),
        }
    }
}

impl std::error::Error for MeasurementError {}

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
// SAFETY: `Session` is shared with the IOProc: the counters are atomics, and `samples` is written
// only by the audio thread and read after `AudioDeviceStop` has returned.
unsafe impl Sync for Session {}

impl Session {
    fn record(&self, elapsed_ticks: u64, host_time: u64) {
        let nanos = elapsed_ticks.saturating_mul(self.numer) / self.denom;
        let index = self.sample_count.fetch_add(1, Ordering::Relaxed);
        // SAFETY: only this thread writes, and `index` is the slot it owns.
        let samples = unsafe { &mut *self.samples.get() };
        if let Some(sample) = samples.get_mut(index as usize) {
            *sample = u32::try_from(nanos).unwrap_or(u32::MAX);
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
pub fn run(seconds: u64, workers: Option<usize>) -> Result<Report, MeasurementError> {
    run_measurement(seconds, workers, true)
}

/// Brief hardware checks use the current device rate without reconfiguring it.
pub fn run_at_device_rate(
    seconds: u64,
    workers: Option<usize>,
) -> Result<Report, MeasurementError> {
    run_measurement(seconds, workers, false)
}

fn measurement_geometry(
    sample_rate: f64,
    seconds: u64,
    require_reference_rate: bool,
) -> Result<(u64, usize), MeasurementError> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(MeasurementError::InvalidSampleRate);
    }
    if require_reference_rate && (sample_rate - TARGET_RATE).abs() > 1.0 {
        return Err(MeasurementError::RateMismatch { rate: sample_rate });
    }
    let budget_ns = (BLOCK_FRAMES as f64 * 1_000_000_000.0 / sample_rate) as u64;
    if budget_ns == 0 {
        return Err(MeasurementError::PeriodTooShort);
    }
    // Storage covers this measurement, including control-loop overshoot and
    // warmup. It is not a project limit, and scales with the actual IO rate.
    let callbacks_per_second = (sample_rate / BLOCK_FRAMES as f64).ceil() as u128;
    let capacity = (u128::from(seconds) + 1)
        .checked_mul(callbacks_per_second)
        .and_then(|count| count.checked_add(u128::from(WARMUP_CALLBACKS)))
        .and_then(|count| usize::try_from(count).ok())
        .ok_or(MeasurementError::DurationTooLong)?;
    Ok((budget_ns, capacity))
}

fn run_measurement(
    seconds: u64,
    workers: Option<usize>,
    require_reference_rate: bool,
) -> Result<Report, MeasurementError> {
    let device = default_output().map_err(|status| DeviceProperty::DefaultOutput.failed(status))?;
    let sample_rate =
        get_f64(device, SAMPLE_RATE).map_err(|status| DeviceProperty::SampleRate.failed(status))?;
    let (budget_ns, capacity) = measurement_geometry(sample_rate, seconds, require_reference_rate)?;
    let original_buffer = get_u32(device, BUFFER_FRAMES)
        .map_err(|status| DeviceProperty::BufferFrames.failed(status))?;
    let original_mute = get_u32(device, PROCESS_MUTE).ok();
    set_u32(device, BUFFER_FRAMES, SCOPE_GLOBAL, BLOCK_FRAMES as u32)
        .or_else(|_| set_u32(device, BUFFER_FRAMES, SCOPE_OUTPUT, BLOCK_FRAMES as u32))
        .map_err(MeasurementError::SetBuffer)?;
    let _restore = Restore {
        device,
        buffer_frames: original_buffer,
        mute: original_mute,
    };
    let actual_buffer = get_u32(device, BUFFER_FRAMES)
        .map_err(|status| DeviceProperty::BufferFrames.failed(status))?;
    if actual_buffer != BLOCK_FRAMES as u32 {
        return Err(MeasurementError::BufferKept(actual_buffer));
    }
    let _ = set_u32(device, PROCESS_MUTE, SCOPE_GLOBAL, 1);
    let _ = set_u32(device, PROCESS_MUTE, SCOPE_OUTPUT, 1);

    let mut timebase = Timebase { numer: 0, denom: 1 };
    // SAFETY: `timebase` is a valid out-parameter.
    unsafe { mach_timebase_info(&mut timebase) };
    let budget_ticks = (u128::from(budget_ns) * u128::from(timebase.denom)
        / u128::from(timebase.numer.max(1))) as u64;

    let workgroup = get_pointer(device, WORKGROUP)
        .map_err(|status| DeviceProperty::Workgroup.failed(status))?;
    if workgroup.is_null() {
        return Err(DeviceError::NoWorkgroup.into());
    }

    // SAFETY: `workgroup` is the retained object `get_pointer` returned.
    let workgroup_parallel = unsafe { crate::graph::workgroup_parallel_threads(workgroup) };
    let worker_count = match workers {
        Some(count) => count,
        None => {
            if workgroup_parallel <= 1 {
                // SAFETY: `workgroup` is the retained object `get_pointer` returned, released once
                // on this path.
                unsafe { os_release(workgroup) };
                return Err(MeasurementError::ParallelThreads(workgroup_parallel));
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

    let mut proc_id: AudioDeviceIOProcID = None;
    // SAFETY: the boxed `session` outlives the IOProc, which is destroyed below before the box
    // drops.
    let status = unsafe {
        AudioDeviceCreateIOProcID(
            device,
            Some(io_proc),
            std::ptr::from_ref(&*session).cast_mut().cast(),
            NonNull::from(&mut proc_id),
        )
    };
    if status != 0 {
        drop(workers);
        // SAFETY: `workgroup` is the retained object `get_pointer` returned, released once on this
        // path.
        unsafe { os_release(workgroup) };
        return Err(MeasurementError::CreateIoProc(status));
    }
    // SAFETY: `proc_id` is the IOProc just created on this device.
    let status = unsafe { AudioDeviceStart(device, proc_id) };
    if status != 0 {
        // SAFETY: `proc_id` is the IOProc just created on this device, which never started.
        unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
        drop(workers);
        // SAFETY: `workgroup` is the retained object `get_pointer` returned, released once on this
        // path.
        unsafe { os_release(workgroup) };
        return Err(MeasurementError::StartIo(status));
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

    // SAFETY: `proc_id` is the IOProc created and started above.
    unsafe { AudioDeviceStop(device, proc_id) };
    // SAFETY: `proc_id` is the IOProc created above, stopped on the line before.
    unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
    let failures = session.engine.workgroup_failures();
    let policy_failures = session.engine.policy_failures();
    drop(workers);
    // SAFETY: `workgroup` is the retained object `get_pointer` returned; the workers that joined it
    // have been dropped, and it is released once.
    unsafe { os_release(workgroup) };

    let count = session.sample_count.load(Ordering::Relaxed) as usize;
    // SAFETY: the IOProc was stopped and destroyed above, so nothing writes the samples any more.
    let samples = unsafe { &*session.samples.get() };
    let measured = samples
        .get(WARMUP_CALLBACKS as usize..count.min(samples.len()))
        .unwrap_or(&[]);
    let mut sorted = measured.to_vec();
    sorted.sort_unstable();
    let percentile = |p: f64| -> u64 {
        let last = sorted.len().saturating_sub(1);
        let index = (last as f64 * p).round() as usize;
        sorted.get(index.min(last)).copied().map_or(0, u64::from)
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

unsafe extern "C-unwind" fn io_proc(
    _device: u32,
    _now: NonNull<AudioTimeStamp>,
    _input_data: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output_data: NonNull<AudioBufferList>,
    output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the boxed `Session` given to `AudioDeviceCreateIOProcID`, which outlives
    // the IOProc.
    let session = unsafe { &*(client as *const Session) };
    // SAFETY: `mach_absolute_time` has no preconditions.
    let started = unsafe { mach_absolute_time() };
    let _energy = session.engine.render_block();
    // SAFETY: HAL passes this call's output list.
    unsafe { silence(output_data.as_ptr()) };
    // SAFETY: `mach_absolute_time` has no preconditions.
    let elapsed = unsafe { mach_absolute_time() }.wrapping_sub(started);
    // SAFETY: HAL provides a live timestamp for the duration of the callback.
    let host_time = unsafe { output_time.as_ref().mHostTime };
    session.record(elapsed, host_time);
    0
}

/// Zeroes every buffer of an IOProc's output list.
///
/// # Safety
/// `list` is null or the output list of the current IOProc call.
unsafe fn silence(list: *mut AudioBufferList) {
    if list.is_null() {
        return;
    }
    // SAFETY: CoreAudio passes a live variable-length buffer list for this callback.
    unsafe {
        let count = (*list).mNumberBuffers as isize;
        let first = std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>();
        for index in 0..count {
            let buffer = &*first.offset(index);
            if !buffer.mData.is_null() && buffer.mDataByteSize > 0 {
                std::ptr::write_bytes(buffer.mData.cast::<u8>(), 0, buffer.mDataByteSize as usize);
            }
        }
    }
}

fn stop_requested() -> bool {
    STOP.load(Ordering::Relaxed) != 0
}

static STOP: AtomicU32 = AtomicU32::new(0);

pub fn install_stop_handler() {
    // SAFETY: `signal` installs an async-signal-safe handler that only stores an atomic.
    unsafe {
        libc::signal(libc::SIGINT, handle_stop as *const () as libc::sighandler_t);
    }
}

unsafe extern "C" fn handle_stop(_signal: i32) {
    STOP.store(1, Ordering::Relaxed);
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
