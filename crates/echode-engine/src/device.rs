//! Shared CoreAudio output queries and legacy HAL declarations.
//!
//! The Phase 0 measurement is only compiled with the `spike` feature.

use std::ffi::c_void;

#[cfg(feature = "spike")]
#[path = "device_measurement.rs"]
pub(crate) mod measurement;

const SYSTEM_OBJECT: u32 = 1;
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const SCOPE_OUTPUT: u32 = u32::from_be_bytes(*b"outp");
const ELEMENT_MAIN: u32 = 0;
const DEFAULT_OUTPUT: u32 = u32::from_be_bytes(*b"dOut");
const SAMPLE_RATE: u32 = u32::from_be_bytes(*b"nsrt");
const BUFFER_FRAMES: u32 = u32::from_be_bytes(*b"fsiz");
const WORKGROUP: u32 = u32::from_be_bytes(*b"oswg");

#[repr(C)]
struct PropAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[cfg(feature = "spike")]
#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[cfg(feature = "spike")]
#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

#[cfg(feature = "spike")]
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

#[cfg(feature = "spike")]
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
    #[cfg(feature = "spike")]
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const PropAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
    #[cfg(feature = "spike")]
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
    #[cfg(feature = "spike")]
    fn AudioDeviceDestroyIOProcID(device: u32, proc_id: *mut c_void) -> i32;
    #[cfg(feature = "spike")]
    fn AudioDeviceStart(device: u32, proc_id: *mut c_void) -> i32;
    #[cfg(feature = "spike")]
    fn AudioDeviceStop(device: u32, proc_id: *mut c_void) -> i32;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
    #[cfg(feature = "spike")]
    fn mach_absolute_time() -> u64;
    fn os_release(object: *mut c_void);
    #[cfg(feature = "spike")]
    fn signal(sig: i32, handler: unsafe extern "C" fn(i32)) -> *mut c_void;
}
