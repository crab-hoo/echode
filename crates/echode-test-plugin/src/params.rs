//! `dev.echode.test-params`: an effect whose output is `input * gain * mode`.
//!
//! Gain is continuous in 0..=2. Mode is stepped: 0 keeps the level, 1 halves
//! it, 2 mutes. A hidden and a read-only parameter test what a host shows.
//! One parameter reports an inverted range, which a host must not offer.
//! The helper hosts one plugin per process, so the values are process-wide.
//!
//! `ECHODE_TEST_FAULT=params-wait:<socket>` holds every parameter listing on
//! the plugin's main thread until the test answers on that Unix socket.

use std::ffi::{CStr, c_char};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::extensions::prelude::*;
use clack_plugin::prelude::*;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_param_value, clap_input_events,
    clap_output_events,
};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN,
    CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED, clap_param_info, clap_plugin_params,
};
use clap_sys::plugin::clap_plugin;
use clap_sys::string_sizes::{CLAP_NAME_SIZE, CLAP_PATH_SIZE};

use crate::TestGain;

pub const GAIN: u32 = 1;
pub const MODE: u32 = 2;
const HIDDEN: u32 = 3;
const READ_ONLY: u32 = 4;
const INVERTED: u32 = 5;

static GAIN_BITS: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000); // 1.0
static MODE_BITS: AtomicU64 = AtomicU64::new(0);
static HIDDEN_BITS: AtomicU64 = AtomicU64::new(0);
static LISTING_WAIT: OnceLock<String> = OnceLock::new();

pub fn hold_listings(socket: &str) {
    let _ = LISTING_WAIT.set(socket.to_string());
}

/// Tells the test that a listing started, then waits for its release byte.
/// A test that already ended closes the socket, and the listing goes on.
fn wait_for_release() {
    use std::io::{Read, Write};
    let Some(path) = LISTING_WAIT.get() else {
        return;
    };
    let Ok(mut socket) = std::os::unix::net::UnixStream::connect(path) else {
        return;
    };
    let mut release = [0];
    let _ = socket
        .write_all(&[1])
        .and_then(|()| socket.read_exact(&mut release));
}

struct Spec {
    id: u32,
    flags: u32,
    name: &'static str,
    module: &'static str,
    min: f64,
    max: f64,
    default: f64,
}

const SPECS: [Spec; 5] = [
    Spec {
        id: GAIN,
        flags: CLAP_PARAM_IS_AUTOMATABLE,
        name: "Gain",
        module: "Output",
        min: 0.0,
        max: 2.0,
        default: 1.0,
    },
    Spec {
        id: MODE,
        flags: CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM | CLAP_PARAM_IS_AUTOMATABLE,
        name: "Mode",
        module: "Output",
        min: 0.0,
        max: 2.0,
        default: 0.0,
    },
    Spec {
        id: HIDDEN,
        flags: CLAP_PARAM_IS_HIDDEN,
        name: "Internal",
        module: "",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    Spec {
        id: READ_ONLY,
        flags: CLAP_PARAM_IS_READONLY,
        name: "Meter",
        module: "",
        min: 0.0,
        max: 1.0,
        default: 0.0,
    },
    Spec {
        id: INVERTED,
        flags: 0,
        name: "Inverted",
        module: "",
        min: 1.0,
        max: 0.0,
        default: 0.5,
    },
];

fn value(id: u32) -> Option<f64> {
    let bits = match id {
        GAIN => &GAIN_BITS,
        MODE => &MODE_BITS,
        HIDDEN => &HIDDEN_BITS,
        READ_ONLY => return Some(0.0),
        INVERTED => return Some(0.5),
        _ => return None,
    };
    Some(f64::from_bits(bits.load(Ordering::Acquire)))
}

/// A value the user set in the plugin's window.
pub fn set_by_user(id: u32, value: f64) {
    set(id, value);
}

/// Stores a value inside its range. Read-only and unknown ids are ignored.
fn set(id: u32, value: f64) {
    if !value.is_finite() {
        return;
    }
    let (bits, clamped) = match id {
        GAIN => (&GAIN_BITS, value.clamp(0.0, 2.0)),
        MODE => (&MODE_BITS, value.round().clamp(0.0, 2.0)),
        HIDDEN => (&HIDDEN_BITS, value.clamp(0.0, 1.0)),
        _ => return,
    };
    bits.store(clamped.to_bits(), Ordering::Release);
}

fn factor(gain: f64, mode: f64) -> f32 {
    let mode = match mode as u8 {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    };
    (gain * mode) as f32
}

/// Applies each value at its sample, per channel, then keeps the block's end values.
pub fn process(audio: &mut Audio<'_>, events: &InputEvents<'_>) -> Result<(), PluginError> {
    let start = (value(GAIN).unwrap_or(1.0), value(MODE).unwrap_or(0.0));
    let mut end = start;
    for mut port in audio {
        let Some(channels) = port.channels()?.into_f32() else {
            continue;
        };
        for pair in channels {
            let (mut gain, mut mode) = start;
            let mut next = 0usize;
            let mut advance = |frame: usize, gain: &mut f64, mode: &mut f64| {
                while next < events.len() as usize && events[next].header().time() as usize <= frame
                {
                    if let Some(CoreEventSpace::ParamValue(change)) = events[next].as_core_event()
                        && let Some(id) = change.param_id()
                    {
                        match id.get() {
                            GAIN => *gain = change.value().clamp(0.0, 2.0),
                            MODE => *mode = change.value().round().clamp(0.0, 2.0),
                            _ => {}
                        }
                    }
                    next += 1;
                }
            };
            match pair {
                ChannelPair::InputOnly(_) => {}
                ChannelPair::OutputOnly(output) => output.fill(0.0),
                ChannelPair::InputOutput(input, output) => {
                    for (frame, (input, output)) in input.iter().zip(output).enumerate() {
                        advance(frame, &mut gain, &mut mode);
                        *output = input * factor(gain, mode);
                    }
                }
                ChannelPair::InPlace(buffer) => {
                    for (frame, sample) in buffer.iter_mut().enumerate() {
                        advance(frame, &mut gain, &mut mode);
                        *sample *= factor(gain, mode);
                    }
                }
            }
            advance(usize::MAX, &mut gain, &mut mode);
            end = (gain, mode);
        }
    }
    set(GAIN, end.0);
    set(MODE, end.1);
    Ok(())
}

/// Gain and mode, as two little-endian `f64`s. The hidden value is not saved.
pub fn save() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&value(GAIN).unwrap_or(1.0).to_le_bytes());
    bytes[8..].copy_from_slice(&value(MODE).unwrap_or(0.0).to_le_bytes());
    bytes
}

pub fn load(bytes: &[u8]) -> bool {
    let Ok(bytes) = <[u8; 16]>::try_from(bytes) else {
        return false;
    };
    set(
        GAIN,
        f64::from_le_bytes(bytes[..8].try_into().expect("8 bytes")),
    );
    set(
        MODE,
        f64::from_le_bytes(bytes[8..].try_into().expect("8 bytes")),
    );
    true
}

#[derive(Copy, Clone)]
pub struct PluginParamsExt;

unsafe impl Extension for PluginParamsExt {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_PARAMS];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

unsafe impl ExtensionImplementation<TestGain> for PluginParamsExt {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_params {
            count: Some(count),
            get_info: Some(get_info),
            get_value: Some(get_value),
            value_to_text: Some(value_to_text),
            text_to_value: Some(text_to_value),
            flush: Some(flush),
        });
}

unsafe extern "C" fn count(_plugin: *const clap_plugin) -> u32 {
    wait_for_release();
    SPECS.len() as u32
}

/// Leaves the last slot as the terminating NUL.
fn copy_text(target: &mut [c_char], text: &str) {
    let room = target.len().saturating_sub(1);
    for (slot, byte) in target.iter_mut().zip(text.bytes().take(room)) {
        *slot = byte as c_char;
    }
}

unsafe extern "C" fn get_info(
    _plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    let (Some(spec), false) = (SPECS.get(index as usize), info.is_null()) else {
        return false;
    };
    let mut name = [0; CLAP_NAME_SIZE];
    let mut module = [0; CLAP_PATH_SIZE];
    copy_text(&mut name, spec.name);
    copy_text(&mut module, spec.module);
    // SAFETY: the host passed writable storage for one description.
    unsafe {
        *info = clap_param_info {
            id: spec.id,
            flags: spec.flags,
            cookie: std::ptr::null_mut(),
            name,
            module,
            min_value: spec.min,
            max_value: spec.max,
            default_value: spec.default,
        };
    }
    true
}

unsafe extern "C" fn get_value(_plugin: *const clap_plugin, id: u32, out: *mut f64) -> bool {
    let (Some(current), false) = (value(id), out.is_null()) else {
        return false;
    };
    // SAFETY: the host passed a writable value.
    unsafe { *out = current };
    true
}

fn text_for(id: u32, value: f64) -> Option<String> {
    Some(match id {
        GAIN => format!("{value:.2}x"),
        MODE => match value.round() as i64 {
            0 => "Clean".to_string(),
            1 => "Half".to_string(),
            _ => "Mute".to_string(),
        },
        HIDDEN | READ_ONLY | INVERTED => format!("{value:.2}"),
        _ => return None,
    })
}

unsafe extern "C" fn value_to_text(
    _plugin: *const clap_plugin,
    id: u32,
    value: f64,
    out: *mut c_char,
    capacity: u32,
) -> bool {
    let (Some(text), false) = (text_for(id, value), out.is_null() || capacity == 0) else {
        return false;
    };
    // SAFETY: the host passed `capacity` writable bytes.
    let target = unsafe { std::slice::from_raw_parts_mut(out, capacity as usize) };
    target.fill(0);
    copy_text(target, &text);
    true
}

unsafe extern "C" fn text_to_value(
    _plugin: *const clap_plugin,
    id: u32,
    text: *const c_char,
    out: *mut f64,
) -> bool {
    if text.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: the host passed a NUL-terminated string.
    let Ok(text) = unsafe { CStr::from_ptr(text) }.to_str() else {
        return false;
    };
    let parsed = match (id, text) {
        (MODE, "Clean") => Some(0.0),
        (MODE, "Half") => Some(1.0),
        (MODE, "Mute") => Some(2.0),
        _ => text.trim_end_matches('x').parse().ok(),
    };
    let Some(parsed) = parsed.filter(|value: &f64| value.is_finite()) else {
        return false;
    };
    // SAFETY: the host passed a writable value.
    unsafe { *out = parsed };
    true
}

/// Applies host values while the plugin is not processing a block, and
/// reports a change made in its window.
unsafe extern "C" fn flush(
    _plugin: *const clap_plugin,
    input: *const clap_input_events,
    output: *const clap_output_events,
) {
    // SAFETY: the host passes its output list for this call, or null.
    unsafe { crate::gui::emit_raw(output) };
    let Some(input) = (unsafe { input.as_ref() }) else {
        return;
    };
    let (Some(size), Some(get)) = (input.size, input.get) else {
        return;
    };
    // SAFETY: the host owns the list for this call and gives a valid count.
    for index in 0..unsafe { size(input) } {
        let header = unsafe { get(input, index) };
        let Some(event) = (unsafe { header.as_ref() }) else {
            continue;
        };
        if event.space_id != CLAP_CORE_EVENT_SPACE_ID || event.type_ != CLAP_EVENT_PARAM_VALUE {
            continue;
        }
        // SAFETY: the header says this is a core parameter-value event.
        let change = unsafe { &*header.cast::<clap_event_param_value>() };
        set(change.param_id, change.value);
    }
}
