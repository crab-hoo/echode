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
use std::time::{Duration, Instant};

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
    pub workers: usize,
    pub passed: bool,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        writeln!(f, "workers: {}", self.workers)?;
        writeln!(f, "result: {}", if self.passed { "pass" } else { "fail" })?;
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

pub fn run(seconds: u64, workers: usize) -> Result<Report, String> {
    let device = default_output().map_err(|status| format!("default output device: {status}"))?;
    let sample_rate =
        get_f64(device, SAMPLE_RATE).map_err(|status| format!("sample rate: {status}"))?;
    if (sample_rate - TARGET_RATE).abs() > 1.0 {
        return Err(format!(
            "output device is at {sample_rate} Hz; this spike requires {TARGET_RATE}"
        ));
    }
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
    let budget_ticks = ((BLOCK_FRAMES as u128) * 1_000_000_000 * u128::from(timebase.denom)
        / (TARGET_RATE as u128 * u128::from(timebase.numer.max(1)))) as u64;

    let workgroup =
        get_pointer(device, WORKGROUP).map_err(|status| format!("workgroup: {status}"))?;
    if workgroup.is_null() {
        return Err("device did not return an IO workgroup".to_string());
    }

    let worker_count = workers;
    let engine = Arc::new(Engine::new(worker_count));
    let workers = engine.spawn_workers(workgroup);
    let capacity = usize::try_from(seconds).unwrap_or(1) * 800 + 4_000;
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

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut next_report = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !stop_requested() {
        thread::sleep(Duration::from_millis(200));
        if Instant::now() >= next_report {
            eprintln!(
                "callbacks={} late={} gaps={} max={:.3} ms",
                session.sample_count.load(Ordering::Relaxed),
                session.late.load(Ordering::Relaxed),
                session.gaps.load(Ordering::Relaxed),
                session.max_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
            );
            next_report = Instant::now() + Duration::from_secs(30);
        }
    }

    unsafe { AudioDeviceStop(device, proc_id) };
    unsafe { AudioDeviceDestroyIOProcID(device, proc_id) };
    let failures = session.engine.workgroup_failures();
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
    let passed = late == 0
        && gaps == 0
        && failures == 0
        && actual_buffer == BLOCK_FRAMES as u32
        && !sorted.is_empty();
    Ok(Report {
        sample_rate,
        buffer_frames: actual_buffer,
        callbacks: count as u32,
        late,
        gaps,
        warmup_late: session.warmup_late.load(Ordering::Relaxed),
        max_ns: session.max_ns.load(Ordering::Relaxed),
        p50_ns: percentile(0.50),
        p99_ns: percentile(0.99),
        budget_ns: (BLOCK_FRAMES as u64) * 1_000_000_000 / TARGET_RATE as u64,
        workgroup_failures: failures,
        workers: worker_count,
        passed,
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
