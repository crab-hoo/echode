//! `clap.gui` for `dev.echode.test-params`. The view draws nothing and never
//! touches the window it is given.
//!
//! With `ECHODE_TEST_FAULT=gui:<socket>` the view connects to that Unix socket
//! when it is created and writes one line per window call: `create`, `parent`,
//! `show`, `hide` and `destroy`. Lines from the test act like a user in the
//! window: `begin <id>`, `value <id> <value>` and `end <id>` report a gesture
//! or value through the host's `request_flush`, `dirty` marks the state
//! changed, and `closed` reports the window closed as a floating window would.
//! Each command is answered with `ok` once the plugin has written its event,
//! or at once for `dirty` and `closed`, which run later on the main thread.

use std::ffi::{CStr, c_char};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::{
    ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::events::io::OutputEvents;
use clack_plugin::extensions::prelude::*;
use clack_plugin::utils::ClapId;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END,
    CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_gesture, clap_event_param_value,
    clap_output_events,
};
use clap_sys::ext::gui::{
    CLAP_EXT_GUI, CLAP_WINDOW_API_COCOA, clap_gui_resize_hints, clap_host_gui, clap_plugin_gui,
    clap_window,
};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_host_params};
use clap_sys::plugin::clap_plugin;

use crate::TestGain;

const NONE: u8 = 0;
const BEGIN: u8 = 1;
const VALUE: u8 = 2;
const END: u8 = 3;

static REMOTE: OnceLock<String> = OnceLock::new();
static HOST: OnceLock<HostSharedHandle<'static>> = OnceLock::new();
/// One event the test asked for. The test waits for `ok` before the next,
/// so this fixture needs no queue.
static PENDING: AtomicU8 = AtomicU8::new(NONE);
static PENDING_ID: AtomicU32 = AtomicU32::new(0);
static PENDING_VALUE: AtomicU64 = AtomicU64::new(0);
/// `mark_dirty` is a main-thread call; the remote asks for a callback.
pub static DIRTY: AtomicBool = AtomicBool::new(false);
/// `closed` is a main-thread call too.
pub static CLOSE: AtomicBool = AtomicBool::new(false);
static LINK: Mutex<Option<Link>> = Mutex::new(None);

struct Link {
    stream: UnixStream,
    stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
}

pub fn remote_at(path: &str) {
    let _ = REMOTE.set(path.to_string());
}

/// Kept for the remote, which asks for `flush` and main-thread callbacks.
pub fn remember_host(host: HostSharedHandle<'_>) {
    // SAFETY: the remote stops in `destroy`, before the host releases the plugin.
    let _ = HOST.set(unsafe { host.with_arbitrary_lifetime() });
}

fn report(line: &str) {
    if let Some(link) = LINK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        let _ = (&link.stream).write_all(format!("{line}\n").as_bytes());
    }
}

/// The event the test asked for, taken once by `process` or `flush`.
fn take() -> Option<(u8, u32, f64)> {
    let kind = PENDING.load(Ordering::Acquire);
    if kind == NONE {
        return None;
    }
    let id = PENDING_ID.load(Ordering::Acquire);
    let value = f64::from_bits(PENDING_VALUE.load(Ordering::Acquire));
    PENDING.store(NONE, Ordering::Release);
    Some((kind, id, value))
}

/// Writes the pending event to a `process` call's output.
pub fn emit(output: &mut OutputEvents<'_>) {
    let Some((kind, id, value)) = take() else {
        return;
    };
    let Some(id) = ClapId::from_raw(id) else {
        return;
    };
    let _ = match kind {
        BEGIN => output.try_push(ParamGestureBeginEvent::new(0, id)),
        END => output.try_push(ParamGestureEndEvent::new(0, id)),
        _ => output.try_push(ParamValueEvent::new(0, id, Pckn::match_all(), value)),
    };
}

/// Writes the pending event to a `flush` call's output.
///
/// # Safety
/// `output` is null or the host's output list for this call.
pub unsafe fn emit_raw(output: *const clap_output_events) {
    // SAFETY: the caller passes null or the host's output list for this call.
    let Some(output) = (unsafe { output.as_ref() }) else {
        return;
    };
    let Some(push) = output.try_push else {
        return;
    };
    let Some((kind, id, value)) = take() else {
        return;
    };
    let header = |type_: u16, size: usize| clap_event_header {
        size: size as u32,
        time: 0,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_,
        flags: 0,
    };
    if kind == VALUE {
        let event = clap_event_param_value {
            header: header(CLAP_EVENT_PARAM_VALUE, size_of::<clap_event_param_value>()),
            param_id: id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        // SAFETY: the host copies the event during the call.
        unsafe { push(output, &event.header) };
    } else {
        let type_ = if kind == BEGIN {
            CLAP_EVENT_PARAM_GESTURE_BEGIN
        } else {
            CLAP_EVENT_PARAM_GESTURE_END
        };
        let event = clap_event_param_gesture {
            header: header(type_, size_of::<clap_event_param_gesture>()),
            param_id: id,
        };
        // SAFETY: the host copies the event during the call.
        unsafe { push(output, &event.header) };
    }
}

#[derive(Copy, Clone)]
struct SeenHostParams(RawExtension<HostExtensionSide, clap_host_params>);

// SAFETY: `CLAP_EXT_PARAMS` is the official identifier of the extension this type stands for.
unsafe impl Extension for SeenHostParams {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_PARAMS];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: `IDENTIFIERS` names the params extension, whose host table is `clap_host_params`,
        // so the raw extension clack found has this type.
        Self(unsafe { raw.cast() })
    }
}

#[derive(Copy, Clone)]
struct SeenHostGui(RawExtension<HostExtensionSide, clap_host_gui>);

// SAFETY: `CLAP_EXT_GUI` is the official identifier of the extension this type stands for.
unsafe impl Extension for SeenHostGui {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_GUI];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: `IDENTIFIERS` names the GUI extension, whose host table is `clap_host_gui`, so
        // the raw extension clack found has this type.
        Self(unsafe { raw.cast() })
    }
}

/// Main thread: tells the host the window is gone, as a floating window would.
pub fn report_closed() {
    let Some(gui) = HOST
        .get()
        .and_then(|host| host.get_extension::<SeenHostGui>())
    else {
        return;
    };
    let host = gui.0.host_ptr();
    // SAFETY: called from the plugin's main-thread callback.
    if let Some(closed) = unsafe { gui.0.as_ptr().as_ref().closed } {
        // SAFETY: `closed` is the host's own function, called from the plugin's main thread with
        // the host pointer it gave this plugin.
        unsafe { closed(host.as_ptr(), true) };
    }
}

fn request_flush() {
    let Some(host) = HOST.get() else {
        return;
    };
    let Some(params) = host.get_extension::<SeenHostParams>() else {
        return;
    };
    let host_ptr = params.0.host_ptr();
    // SAFETY: CLAP's `request_flush` may be called from any thread.
    if let Some(flush) = unsafe { params.0.as_ptr().as_ref().request_flush } {
        // SAFETY: `request_flush` is the host's own function, which CLAP lets any thread call with
        // the host pointer it gave this plugin.
        unsafe { flush(host_ptr.as_ptr()) };
    }
}

/// Reads the test's commands until the view is destroyed.
fn serve(stream: UnixStream, stop: Arc<AtomicBool>) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else {
            return;
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        let id = words.get(1).and_then(|word| word.parse::<u32>().ok());
        let value = words.get(2).and_then(|word| word.parse::<f64>().ok());
        let kind = match (words.first().copied(), id, value) {
            (Some("begin"), Some(_), _) => BEGIN,
            (Some("value"), Some(id), Some(value)) => {
                crate::params::set_by_user(id, value);
                VALUE
            }
            (Some("end"), Some(_), _) => END,
            (Some(word @ ("dirty" | "closed")), _, _) => {
                if word == "dirty" {
                    DIRTY.store(true, Ordering::Release);
                } else {
                    CLOSE.store(true, Ordering::Release);
                }
                if let Some(host) = HOST.get() {
                    host.request_callback();
                }
                let _ = (&stream).write_all(b"ok\n");
                continue;
            }
            _ => continue,
        };
        PENDING_ID.store(id.unwrap_or(0), Ordering::Release);
        PENDING_VALUE.store(value.unwrap_or(0.0).to_bits(), Ordering::Release);
        PENDING.store(kind, Ordering::Release);
        request_flush();
        while PENDING.load(Ordering::Acquire) != NONE {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = (&stream).write_all(b"ok\n");
    }
}

fn connect() {
    let Some(path) = REMOTE.get() else {
        return;
    };
    let Ok(stream) = UnixStream::connect(path) else {
        return;
    };
    let Ok(reading) = stream.try_clone() else {
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let reader = std::thread::spawn(move || serve(reading, flag));
    *LINK.lock().unwrap_or_else(|error| error.into_inner()) = Some(Link {
        stream,
        stop,
        reader,
    });
}

fn disconnect() {
    let link = LINK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(link) = link {
        link.stop.store(true, Ordering::Release);
        let _ = link.stream.shutdown(std::net::Shutdown::Both);
        let _ = link.reader.join();
    }
}

#[derive(Copy, Clone)]
pub struct PluginGuiExt;

// SAFETY: `CLAP_EXT_GUI` is the official identifier of the extension this type stands for.
unsafe impl Extension for PluginGuiExt {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_GUI];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_gui`, the `#[repr(C)]` table CLAP declares for
// that identifier.
unsafe impl ExtensionImplementation<TestGain> for PluginGuiExt {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_gui {
            is_api_supported: Some(is_api_supported),
            get_preferred_api: Some(get_preferred_api),
            create: Some(create),
            destroy: Some(destroy),
            set_scale: Some(set_scale),
            get_size: Some(get_size),
            can_resize: Some(can_resize),
            get_resize_hints: Some(get_resize_hints),
            adjust_size: Some(adjust_size),
            set_size: Some(set_size),
            set_parent: Some(set_parent),
            set_transient: Some(set_transient),
            suggest_title: Some(suggest_title),
            show: Some(show),
            hide: Some(hide),
        });
}

fn cocoa(api: *const c_char) -> bool {
    // SAFETY: the host passes a NUL-terminated API name.
    !api.is_null() && unsafe { CStr::from_ptr(api) } == CLAP_WINDOW_API_COCOA
}

unsafe extern "C" fn is_api_supported(
    _plugin: *const clap_plugin,
    api: *const c_char,
    floating: bool,
) -> bool {
    !floating && cocoa(api)
}

unsafe extern "C" fn get_preferred_api(
    _plugin: *const clap_plugin,
    api: *mut *const c_char,
    floating: *mut bool,
) -> bool {
    if api.is_null() || floating.is_null() {
        return false;
    }
    // SAFETY: the host passed writable storage for both.
    unsafe {
        *api = CLAP_WINDOW_API_COCOA.as_ptr();
        *floating = false;
    }
    true
}

unsafe extern "C" fn create(
    _plugin: *const clap_plugin,
    api: *const c_char,
    floating: bool,
) -> bool {
    if floating || !cocoa(api) {
        return false;
    }
    connect();
    report("create");
    true
}

unsafe extern "C" fn destroy(_plugin: *const clap_plugin) {
    report("destroy");
    disconnect();
}

unsafe extern "C" fn set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
    false
}

unsafe extern "C" fn get_size(
    _plugin: *const clap_plugin,
    width: *mut u32,
    height: *mut u32,
) -> bool {
    if width.is_null() || height.is_null() {
        return false;
    }
    // SAFETY: the host passed writable storage for both.
    unsafe {
        *width = 320;
        *height = 200;
    }
    true
}

unsafe extern "C" fn can_resize(_plugin: *const clap_plugin) -> bool {
    false
}

unsafe extern "C" fn get_resize_hints(
    _plugin: *const clap_plugin,
    _hints: *mut clap_gui_resize_hints,
) -> bool {
    false
}

unsafe extern "C" fn adjust_size(_plugin: *const clap_plugin, _w: *mut u32, _h: *mut u32) -> bool {
    false
}

unsafe extern "C" fn set_size(_plugin: *const clap_plugin, _width: u32, _height: u32) -> bool {
    false
}

unsafe extern "C" fn set_parent(_plugin: *const clap_plugin, window: *const clap_window) -> bool {
    // SAFETY: the host passes a window description or null.
    let Some(window) = (unsafe { window.as_ref() }) else {
        return false;
    };
    // SAFETY: a Cocoa window carries its view in the union.
    if !cocoa(window.api) || unsafe { window.specific.cocoa }.is_null() {
        return false;
    }
    report("parent");
    true
}

unsafe extern "C" fn set_transient(
    _plugin: *const clap_plugin,
    _window: *const clap_window,
) -> bool {
    false
}

unsafe extern "C" fn suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

unsafe extern "C" fn show(_plugin: *const clap_plugin) -> bool {
    report("show");
    true
}

unsafe extern "C" fn hide(_plugin: *const clap_plugin) -> bool {
    report("hide");
    true
}
