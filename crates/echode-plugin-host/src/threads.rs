//! Main run loop, one realtime processing thread, and one control thread.
//!
//! The processing thread never waits for `deactivate` / `activate`. Those run
//! on the main thread. A shared alarm wakes the processing thread both for a
//! parent block and for a restart that arrives while the transport is idle.

use crate::events::write_input;
use crate::gui::GuiRequests;
use crate::relay::{RelayOutput, RelayRing};
use crate::{HostError, gui, load_entry};
use clack_host::events::io::{EventBuffer, InputEvents};
use clack_host::events::{
    Pckn,
    event_types::{MidiEvent, NoteChokeEvent, ParamValueEvent},
};
use clack_host::extensions::prelude::{
    Extension, ExtensionImplementation, HostExtensionSide, HostWrapper, RawExtension,
    RawExtensionImplementation,
};
use clack_host::prelude::*;
use clack_host::utils::ClapId;
use clap_sys::ext::audio_ports::{
    CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_host_gui};
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE, clap_note_port_info,
    clap_plugin_note_ports,
};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, CLAP_PARAM_IS_READONLY, CLAP_PARAM_RESCAN_ALL, CLAP_PARAM_RESCAN_INFO,
    clap_host_params, clap_param_clear_flags, clap_param_info, clap_param_rescan_flags,
    clap_plugin_params,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state, clap_plugin_state};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};
use echode_realtime::Event;
use echode_sandbox::{
    CMD_ADOPT_MAPPING, CMD_PROCESS, CMD_RESET_PROCESS, CMD_SHUTDOWN, ControlMessage, FanIn,
    GuiReport, MappedRegion, ParamChange, ParamEvent, ParamInfo, ParamRelay, ParamValue,
    RegionError, Socket, SyncFlags, wake_one,
};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopCommonModes,
};
use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread::{self, ThreadId};

const OP_RESTART: u8 = 1;
const OP_ARM: u8 = 2;
const STATE_SAVE: u8 = 1;
const STATE_LOAD: u8 = 2;
const PREPARE_CAPACITY: u8 = 3;
const PARAM_LIST: u8 = 4;
const PARAM_READ: u8 = 5;
const PARAM_RELAY: u8 = 6;
const GUI_SHOW: u8 = 7;
const GUI_CLOSE: u8 = 8;

/// Creating a plugin's view can load large resources the first time.
const WINDOW_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
const STATE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

const HANDOFF_EMPTY: u8 = 0;
const HANDOFF_FULL: u8 = 1;
const HANDOFF_BUSY: u8 = 2;

thread_local! {
    static INSTANCE: Cell<*mut PluginInstance<EchodeHost>> = const { Cell::new(ptr::null_mut()) };
    /// Set for the whole processing thread, including restarts between blocks.
    static ON_AUDIO: Cell<bool> = const { Cell::new(false) };
    /// Flags by parameter id, read on the main thread for relays. Cleared
    /// when the plugin rescans its descriptions.
    static PARAM_FLAGS: RefCell<Option<HashMap<u32, u32>>> = const { RefCell::new(None) };
}

struct AudioFlag;

impl AudioFlag {
    fn enter() -> Self {
        ON_AUDIO.with(|flag| flag.set(true));
        Self
    }
}

impl Drop for AudioFlag {
    fn drop(&mut self) {
        ON_AUDIO.with(|flag| flag.set(false));
    }
}

/// One stopped processor, handed between the main thread and the audio thread
/// without allocating. `state` is empty, full, or busy.
struct ProcessorHandoff {
    state: AtomicU8,
    slot: UnsafeCell<MaybeUninit<StoppedPluginAudioProcessor<EchodeHost>>>,
}

impl ProcessorHandoff {
    fn empty() -> Self {
        Self {
            state: AtomicU8::new(HANDOFF_EMPTY),
            slot: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn is_empty(&self) -> bool {
        self.state.load(Ordering::Acquire) == HANDOFF_EMPTY
    }

    /// Stores `stopped` when the slot is empty.
    /// On failure the caller still owns `stopped`.
    fn try_store(
        &self,
        stopped: StoppedPluginAudioProcessor<EchodeHost>,
    ) -> Result<(), StoppedPluginAudioProcessor<EchodeHost>> {
        if self
            .state
            .compare_exchange(
                HANDOFF_EMPTY,
                HANDOFF_BUSY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(stopped);
        }
        // SAFETY: BUSY means this thread owns the slot until it publishes FULL.
        unsafe { (*self.slot.get()).write(stopped) };
        self.state.store(HANDOFF_FULL, Ordering::Release);
        Ok(())
    }

    fn try_take(&self) -> Option<StoppedPluginAudioProcessor<EchodeHost>> {
        if self
            .state
            .compare_exchange(
                HANDOFF_FULL,
                HANDOFF_BUSY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return None;
        }
        // SAFETY: BUSY means this thread owns the initialized slot until EMPTY.
        let stopped = unsafe { (*self.slot.get()).assume_init_read() };
        self.state.store(HANDOFF_EMPTY, Ordering::Release);
        Some(stopped)
    }
}

struct Bridge {
    source: AtomicPtr<CFRunLoopSource>,
    run_loop: AtomicPtr<CFRunLoop>,
    root: Arc<MappedRegion>,
    current: AtomicPtr<Arc<MappedRegion>>,
    pending: AtomicPtr<Arc<MappedRegion>>,
    retired: AtomicPtr<Arc<MappedRegion>>,
    pending_buffers: AtomicPtr<PluginBuffers>,
    retired_buffers: AtomicPtr<PluginBuffers>,
    /// The processing thread has allocated its first buffers and can adopt replacements.
    buffers_live: AtomicBool,
    switched: AtomicU64,
    cancel_pending: AtomicBool,
    restart_request: AtomicU64,
    restart_ack: AtomicU64,
    want_process: AtomicBool,
    holds_started: AtomicBool,
    has_plugin: AtomicBool,
    need_start: AtomicBool,
    stop: AtomicBool,
    control_alarm: AtomicU64,
    main_done: AtomicU64,
    restart_for_main: AtomicU64,
    op: AtomicU8,
    processor: ProcessorHandoff,
    callback_gen: AtomicU64,
    callback_seen: AtomicU64,
    process_note: AtomicBool,
    stages: AtomicU32,
    fan: AtomicPtr<FanIn>,
    sample_bits: AtomicU64,
    activation_frames: AtomicU32,
    tail_samples: AtomicU32,
    /// Port channel counts, published before the worker threads start.
    ports: AtomicPtr<PortShape>,
    /// 0 idle, [`STATE_SAVE`], or [`STATE_LOAD`]. The main thread clears it.
    state_kind: AtomicU8,
    state_gen: AtomicU64,
    state_done: AtomicU64,
    /// Bytes to load, or the bytes save produced. Not touched by the audio thread.
    state_bytes: std::sync::Mutex<Vec<u8>>,
    state_error: std::sync::Mutex<Option<String>>,
    plugin_dirty: AtomicBool,
    /// `clap.params`, looked up on the main thread after load. Null when absent.
    params: AtomicPtr<clap_plugin_params>,
    /// Changes the control thread prepared. The processing thread flushes them.
    param_pending: AtomicPtr<ParamBatch>,
    /// Flushed batches, newest first. Only the control thread frees them.
    param_retired: AtomicPtr<ParamBatch>,
    param_generation: AtomicU64,
    param_flushed: AtomicU64,
    /// What the last main-thread parameter read produced.
    param_read: std::sync::Mutex<ParamRead>,
    /// Values and gestures from the plugin's output events.
    relay: RelayRing,
    /// The plugin asked for `flush` while no block may come.
    flush_request: AtomicBool,
    /// The plugin asked the host to read its parameters again.
    rescan_request: AtomicBool,
    /// Window requests the plugin made from any thread.
    gui: GuiRequests,
    /// The user or the plugin closed the window; the parent is told once.
    gui_closed: AtomicBool,
    /// Main-thread AppKit shortcuts, drained only by the control thread.
    gui_commands: std::sync::Mutex<Vec<echode_sandbox::PluginCommand>>,
    // Read after `serve` returns, from the unit test.
    #[allow(dead_code)]
    main_id: std::sync::Mutex<Option<ThreadId>>,
    #[allow(dead_code)]
    audio_id: std::sync::Mutex<Option<ThreadId>>,
    #[allow(dead_code)]
    callbacks: AtomicU64,
}

// SAFETY: the CoreFoundation pointers stay valid until both worker threads have
// been joined. Only the processing thread swaps mapping pointers, and the previous
// mapping is freed only after the processing thread has left it. The stopped
// processor slot is published through `processor.state`, not by aliasing.
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

impl Bridge {
    fn signal(&self) {
        let source = self.source.load(Ordering::Acquire);
        let run_loop = self.run_loop.load(Ordering::Acquire);
        if source.is_null() || run_loop.is_null() {
            return;
        }
        unsafe {
            (*source).signal();
            (*run_loop).wake_up();
        }
    }

    fn bump_audio(&self) {
        if ON_AUDIO.with(|flag| flag.get()) {
            self.root.raise_alarm();
        } else {
            self.root.bump_alarm();
        }
    }

    fn bump_fan(&self) {
        let fan = self.fan.load(Ordering::Acquire);
        if !fan.is_null() {
            // SAFETY: the fan-in mapping outlives both worker threads.
            unsafe { (*fan).bump() };
        }
    }

    fn bump_control(&self) {
        self.control_alarm.fetch_add(1, Ordering::Release);
        wake_one(&self.control_alarm, SyncFlags::THREAD);
    }

    fn on_main(&self) {
        if let Ok(mut slot) = self.main_id.lock() {
            *slot = Some(thread::current().id());
        }
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        let generation = self.callback_gen.load(Ordering::Acquire);
        let seen = self.callback_seen.swap(generation, Ordering::AcqRel);
        if seen != generation {
            INSTANCE.with(|cell| {
                let instance = cell.get();
                if !instance.is_null() {
                    unsafe { (*instance).call_on_main_thread_callback() };
                }
            });
        }
        self.service_state();
        match self.op.swap(0, Ordering::AcqRel) {
            OP_RESTART => self.restart_on_main(),
            OP_ARM => self.arm_on_main(),
            _ => {}
        }
    }

    fn restart_on_main(&self) {
        let want = self.restart_for_main.load(Ordering::Acquire);
        INSTANCE.with(|cell| {
            let instance = cell.get();
            if instance.is_null() {
                return;
            }
            let instance = unsafe { &mut *instance };
            if let Some(stopped) = self.processor.try_take() {
                instance.deactivate(stopped);
                match activate(
                    instance,
                    f64::from_bits(self.sample_bits.load(Ordering::Acquire)),
                    self.activation_frames.load(Ordering::Acquire),
                ) {
                    Ok(stopped) => publish_stopped(&self.processor, stopped, false),
                    Err(error) => {
                        eprintln!("echode-plugin-host: activate after restart failed: {error}");
                        self.stop.store(true, Ordering::Release);
                    }
                }
            }
        });
        self.need_start.store(true, Ordering::Release);
        self.main_done.store(want, Ordering::Release);
        self.bump_audio();
        self.bump_control();
    }

    fn arm_on_main(&self) {
        INSTANCE.with(|cell| {
            let instance = cell.get();
            if instance.is_null() {
                return;
            }
            let instance = unsafe { &mut *instance };
            if self.processor.is_empty() {
                match activate(
                    instance,
                    f64::from_bits(self.sample_bits.load(Ordering::Acquire)),
                    self.activation_frames.load(Ordering::Acquire),
                ) {
                    Ok(stopped) => publish_stopped(&self.processor, stopped, false),
                    Err(error) => {
                        eprintln!("echode-plugin-host: activate failed: {error}");
                        self.stop.store(true, Ordering::Release);
                    }
                }
            }
        });
        self.need_start.store(true, Ordering::Release);
        self.bump_audio();
        self.bump_control();
    }

    /// Runs a queued CLAP state save or load. The audio thread never calls this.
    fn service_state(&self) {
        let kind = self.state_kind.swap(0, Ordering::AcqRel);
        if kind == 0 {
            return;
        }
        let generation = self.state_gen.load(Ordering::Acquire);
        let result = perform_plugin_state(self, kind);
        match result {
            Ok(bytes) => {
                *self
                    .state_bytes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = bytes;
                *self
                    .state_error
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = None;
            }
            Err(error) => {
                *self
                    .state_error
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(error);
            }
        }
        self.state_done.store(generation, Ordering::Release);
    }
}

pub(crate) struct HostShared {
    bridge: Option<Arc<Bridge>>,
}

impl HostShared {
    pub(crate) fn inert() -> Self {
        Self { bridge: None }
    }
}

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {
        let Some(bridge) = &self.bridge else {
            return;
        };
        bridge.restart_request.fetch_add(1, Ordering::Release);
        bridge.bump_audio();
    }

    fn request_process(&self) {
        let Some(bridge) = &self.bridge else {
            return;
        };
        bridge.want_process.store(true, Ordering::Release);
        bridge.process_note.store(true, Ordering::Release);
        bridge.bump_audio();
        bridge.bump_control();
    }

    fn request_callback(&self) {
        let Some(bridge) = &self.bridge else {
            return;
        };
        bridge.callback_gen.fetch_add(1, Ordering::Release);
        bridge.signal();
    }
}

pub(crate) struct EchodeHost;

pub(crate) struct HostMain<'a> {
    bridge: Option<Arc<Bridge>>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

impl<'a> MainThreadHandler<'a> for HostMain<'a> {}

pub(crate) fn host_main(shared: &HostShared) -> HostMain<'_> {
    HostMain {
        bridge: shared.bridge.clone(),
        _lifetime: std::marker::PhantomData,
    }
}

trait MarkDirty {
    fn mark_dirty(&self);
}

impl MarkDirty for HostMain<'_> {
    fn mark_dirty(&self) {
        let Some(bridge) = &self.bridge else {
            return;
        };
        bridge.plugin_dirty.store(true, Ordering::Release);
        bridge.bump_control();
    }
}

#[derive(Copy, Clone)]
struct HostState;

unsafe impl Extension for HostState {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_STATE];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

unsafe impl<H> ExtensionImplementation<H> for HostState
where
    H: for<'a> HostHandlers<MainThread<'a>: MarkDirty>,
{
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_state {
            mark_dirty: Some(host_mark_dirty::<H>),
        });
}

unsafe extern "C" fn host_mark_dirty<H>(host: *const clap_host)
where
    H: for<'a> HostHandlers<MainThread<'a>: MarkDirty>,
{
    unsafe {
        HostWrapper::<H>::handle_main_thread(host, |host| {
            host.mark_dirty();
            Ok(())
        });
    }
}

/// Runs `action` with the bridge of the plugin that called the host.
///
/// # Safety
/// `host` is the pointer this helper gave the plugin.
unsafe fn with_bridge(host: *const clap_host, action: impl FnOnce(&Bridge)) {
    unsafe {
        HostWrapper::<EchodeHost>::handle(host, |wrapper| {
            if let Some(bridge) = &wrapper.shared().bridge {
                action(bridge);
            }
            Ok(())
        });
    }
}

#[derive(Copy, Clone)]
struct HostGui;

unsafe impl Extension for HostGui {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_GUI];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

unsafe impl ExtensionImplementation<EchodeHost> for HostGui {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_gui {
            resize_hints_changed: Some(host_gui_hints),
            request_resize: Some(host_gui_resize),
            request_show: Some(host_gui_show),
            request_hide: Some(host_gui_hide),
            closed: Some(host_gui_closed),
        });
}

/// The helper reads the hints only when the user resizes the window.
unsafe extern "C" fn host_gui_hints(_host: *const clap_host) {}

unsafe extern "C" fn host_gui_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    unsafe {
        with_bridge(host, |bridge| {
            bridge.gui.request_resize(width, height);
            bridge.signal();
        });
    }
    true
}

unsafe extern "C" fn host_gui_show(host: *const clap_host) -> bool {
    unsafe {
        with_bridge(host, |bridge| {
            bridge.gui.request_show();
            bridge.signal();
        });
    }
    true
}

unsafe extern "C" fn host_gui_hide(host: *const clap_host) -> bool {
    unsafe {
        with_bridge(host, |bridge| {
            bridge.gui.request_hide();
            bridge.signal();
        });
    }
    true
}

/// A floating window closed. The host destroys the plugin's view either way.
unsafe extern "C" fn host_gui_closed(host: *const clap_host, _was_destroyed: bool) {
    unsafe {
        with_bridge(host, |bridge| {
            bridge.gui.closed();
            bridge.signal();
        });
    }
}

#[derive(Copy, Clone)]
struct HostParams;

unsafe impl Extension for HostParams {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_PARAMS];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

unsafe impl ExtensionImplementation<EchodeHost> for HostParams {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_host_params {
            rescan: Some(host_params_rescan),
            clear: Some(host_params_clear),
            request_flush: Some(host_params_flush),
        });
}

/// Main thread. The parent reads the list again; changed descriptions also
/// drop the flags the relay uses.
unsafe extern "C" fn host_params_rescan(host: *const clap_host, flags: clap_param_rescan_flags) {
    if flags & (CLAP_PARAM_RESCAN_INFO | CLAP_PARAM_RESCAN_ALL) != 0 {
        PARAM_FLAGS.with(|cache| cache.borrow_mut().take());
    }
    unsafe {
        with_bridge(host, |bridge| {
            bridge.rescan_request.store(true, Ordering::Release);
            bridge.bump_control();
        });
    }
}

/// The host keeps no automation or modulation yet, so there is nothing to clear.
unsafe extern "C" fn host_params_clear(
    _host: *const clap_host,
    _param: u32,
    _flags: clap_param_clear_flags,
) {
}

/// Any thread. The processing thread calls `flush` between blocks, and is
/// woken for it when the transport is stopped.
unsafe extern "C" fn host_params_flush(host: *const clap_host) {
    unsafe {
        with_bridge(host, |bridge| {
            bridge.flush_request.store(true, Ordering::Release);
            bridge.bump_audio();
        });
    }
}

impl HostHandlers for EchodeHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMain<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostState>();
        builder.register::<HostGui>();
        builder.register::<HostParams>();
    }
}

#[cfg(test)]
pub(crate) fn last_report() -> Option<(ThreadId, ThreadId, u64)> {
    LAST_REPORT.lock().ok().and_then(|slot| {
        slot.clone()
            .map(|saved| (saved.main, saved.audio, saved.callbacks))
    })
}

#[cfg(test)]
#[derive(Clone)]
struct SavedReport {
    main: ThreadId,
    audio: ThreadId,
    callbacks: u64,
}

#[cfg(test)]
static LAST_REPORT: std::sync::Mutex<Option<SavedReport>> = std::sync::Mutex::new(None);

pub fn serve(
    shm: &std::path::Path,
    plugin: Option<&std::path::Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    let region = MappedRegion::open(shm)?;
    serve_region(
        region,
        None,
        None,
        plugin,
        plugin_id,
        sample_rate,
        workgroup,
    )
}

pub fn serve_fds(
    shm: OwnedFd,
    control: OwnedFd,
    fan: Option<OwnedFd>,
    plugin: Option<&std::path::Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    let region = MappedRegion::from_fd(shm)?;
    let socket =
        Socket::from_owned(control).map_err(|error| HostError::Plugin(error.to_string()))?;
    let fan = match fan {
        Some(fd) => Some(FanIn::from_fd(fd)?),
        None => None,
    };
    serve_region(
        region,
        Some(socket),
        fan,
        plugin,
        plugin_id,
        sample_rate,
        workgroup,
    )
}

fn serve_region(
    region: MappedRegion,
    mut control: Option<Socket>,
    fan: Option<FanIn>,
    plugin: Option<&std::path::Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    let max_frames = region.layout().max_frames;
    let root = Arc::new(region);
    let region = Box::into_raw(Box::new(Arc::clone(&root)));
    let fan = fan.map(|fan| Box::into_raw(Box::new(fan)));
    let bridge = Arc::new(Bridge {
        source: AtomicPtr::new(ptr::null_mut()),
        run_loop: AtomicPtr::new(ptr::null_mut()),
        root,
        current: AtomicPtr::new(region),
        pending: AtomicPtr::new(ptr::null_mut()),
        retired: AtomicPtr::new(ptr::null_mut()),
        pending_buffers: AtomicPtr::new(ptr::null_mut()),
        retired_buffers: AtomicPtr::new(ptr::null_mut()),
        buffers_live: AtomicBool::new(false),
        switched: AtomicU64::new(0),
        cancel_pending: AtomicBool::new(false),
        restart_request: AtomicU64::new(0),
        restart_ack: AtomicU64::new(0),
        want_process: AtomicBool::new(false),
        holds_started: AtomicBool::new(false),
        has_plugin: AtomicBool::new(plugin.is_some()),
        need_start: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        control_alarm: AtomicU64::new(0),
        main_done: AtomicU64::new(0),
        restart_for_main: AtomicU64::new(0),
        op: AtomicU8::new(0),
        processor: ProcessorHandoff::empty(),
        callback_gen: AtomicU64::new(0),
        callback_seen: AtomicU64::new(0),
        process_note: AtomicBool::new(false),
        stages: AtomicU32::new(1),
        fan: AtomicPtr::new(fan.unwrap_or(ptr::null_mut())),
        sample_bits: AtomicU64::new(sample_rate.to_bits()),
        activation_frames: AtomicU32::new(max_frames.max(1)),
        tail_samples: AtomicU32::new(0),
        ports: AtomicPtr::new(ptr::null_mut()),
        state_kind: AtomicU8::new(0),
        state_gen: AtomicU64::new(0),
        state_done: AtomicU64::new(0),
        state_bytes: std::sync::Mutex::new(Vec::new()),
        state_error: std::sync::Mutex::new(None),
        plugin_dirty: AtomicBool::new(false),
        params: AtomicPtr::new(ptr::null_mut()),
        param_pending: AtomicPtr::new(ptr::null_mut()),
        param_retired: AtomicPtr::new(ptr::null_mut()),
        param_generation: AtomicU64::new(0),
        param_flushed: AtomicU64::new(0),
        param_read: std::sync::Mutex::new(ParamRead::Infos(Vec::new())),
        relay: RelayRing::new(),
        flush_request: AtomicBool::new(false),
        rescan_request: AtomicBool::new(false),
        gui: GuiRequests::default(),
        gui_closed: AtomicBool::new(false),
        gui_commands: std::sync::Mutex::new(Vec::new()),
        main_id: std::sync::Mutex::new(None),
        audio_id: std::sync::Mutex::new(None),
        callbacks: AtomicU64::new(0),
    });

    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: ptr::from_ref(bridge.as_ref()).cast_mut().cast::<c_void>(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(on_source),
    };
    let source = unsafe { CFRunLoopSource::new(None, 0, &raw mut context) }.ok_or_else(|| {
        HostError::Plugin("could not create the main run loop source".to_string())
    })?;
    let run_loop = CFRunLoop::current()
        .ok_or_else(|| HostError::Plugin("the thread has no run loop".to_string()))?;
    // SAFETY: the common-modes symbol is a process-wide constant.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    // Retained below so the pointers stay valid until the worker threads are joined.
    bridge
        .source
        .store(ptr::from_ref(&*source).cast_mut(), Ordering::Release);
    bridge
        .run_loop
        .store(ptr::from_ref(&*run_loop).cast_mut(), Ordering::Release);
    let _kept_source: CFRetained<CFRunLoopSource> = source;
    let _kept_loop: CFRetained<CFRunLoop> = run_loop;

    let mut loaded = match plugin {
        None => None,
        Some(path) => match load_plugin(path, plugin_id, Arc::clone(&bridge)) {
            Ok(plugin) => Some(plugin),
            Err(error) => {
                reject_launch(control.as_mut(), &error.to_string());
                return Err(error);
            }
        },
    };
    if let Some(loaded) = loaded.as_mut() {
        // The plugin is still inactive. Audio ports are only valid to read here,
        // on the main thread, before activate.
        let shape = match query_audio_ports(&loaded.instance) {
            Ok(shape) => shape,
            Err(error) => PortShape {
                inputs: Vec::new(),
                outputs: Vec::new(),
                reset_midi: Vec::new(),
                error: Some(error),
            },
        };
        let rejected = shape.error.clone();
        bridge
            .ports
            .store(Box::into_raw(Box::new(shape)), Ordering::Release);
        if let Some(error) = rejected {
            // Tell the parent before any worker starts. A worker that exits
            // first can close the socket and drop this reason.
            if let Some(socket) = control.as_mut() {
                let _ = socket.send(&ControlMessage::PortsRejected(error));
            }
            bridge.stop.store(true, Ordering::Release);
        } else {
            let stopped = match activate(&mut loaded.instance, sample_rate, max_frames) {
                Ok(stopped) => stopped,
                Err(error) => {
                    reject_launch(control.as_mut(), &error.to_string());
                    return Err(error);
                }
            };
            bridge
                .tail_samples
                .store(query_tail(&loaded.instance), Ordering::Release);
            prepare_event_capacity(&loaded.instance, bridge.root.layout().input_events)
                .map_err(HostError::Plugin)?;
            publish_stopped(&bridge.processor, stopped, false);
            bridge.need_start.store(true, Ordering::Release);
            bridge
                .params
                .store(params_extension(&loaded.instance), Ordering::Release);
            INSTANCE.with(|cell| cell.set(ptr::from_mut(&mut loaded.instance)));
        }
    }

    let workgroup = workgroup.map(|(name, ticks)| (name.to_string(), ticks));
    let bridge_audio = Arc::clone(&bridge);
    let bridge_control = Arc::clone(&bridge);
    let audio_result = thread::scope(|scope| {
        let audio = scope.spawn(|| {
            if let Ok(mut slot) = bridge_audio.audio_id.lock() {
                *slot = Some(thread::current().id());
            }
            let _audio = AudioFlag::enter();
            let result = audio_thread(&bridge_audio, workgroup.as_ref());
            if result.is_err() {
                bridge_audio.stop.store(true, Ordering::Release);
                bridge_audio.bump_control();
                stop_loop(&bridge_audio);
            }
            result
        });
        let control_handle = scope.spawn(|| control_thread(&bridge_control, control));
        bridge.signal();
        while !bridge.stop.load(Ordering::Acquire) {
            gui::run_once(0.05, |command| {
                bridge
                    .gui_commands
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(command);
                bridge.bump_control();
            });
            tick_window(&bridge);
        }
        audio
            .join()
            .expect("processing thread")
            .and(control_handle.join().expect("control thread"))
    });

    if let Some(loaded) = loaded.as_ref() {
        // The plugin's view goes before the plugin is deactivated and destroyed.
        gui::shut_down(loaded.instance.raw_instance());
    }
    INSTANCE.with(|cell| cell.set(ptr::null_mut()));
    if let Some(loaded) = loaded.as_mut()
        && let Some(stopped) = bridge.processor.try_take()
    {
        loaded.instance.deactivate(stopped);
    }
    #[cfg(test)]
    if let (Ok(main), Ok(audio)) = (bridge.main_id.lock(), bridge.audio_id.lock())
        && let (Some(main), Some(audio)) = (*main, *audio)
        && let Ok(mut saved) = LAST_REPORT.lock()
    {
        *saved = Some(SavedReport {
            main,
            audio,
            callbacks: bridge.callbacks.load(Ordering::Relaxed),
        });
    }
    release_maps(&bridge);
    // Both worker threads have been joined, so no batch is still being flushed.
    let unclaimed = bridge.param_pending.swap(ptr::null_mut(), Ordering::AcqRel);
    if !unclaimed.is_null() {
        drop(unsafe { Box::from_raw(unclaimed) });
    }
    reclaim_param_batches(&bridge);
    let ports = bridge.ports.swap(ptr::null_mut(), Ordering::AcqRel);
    if !ports.is_null() {
        drop(unsafe { Box::from_raw(ports) });
    }
    let fan = bridge.fan.swap(ptr::null_mut(), Ordering::AcqRel);
    if !fan.is_null() {
        drop(unsafe { Box::from_raw(fan) });
    }
    audio_result
}

fn release_maps(bridge: &Bridge) {
    let maps = [
        bridge.current.swap(ptr::null_mut(), Ordering::AcqRel),
        bridge.pending.swap(ptr::null_mut(), Ordering::AcqRel),
        bridge.retired.swap(ptr::null_mut(), Ordering::AcqRel),
    ];
    let buffers = [
        bridge
            .pending_buffers
            .swap(ptr::null_mut(), Ordering::AcqRel),
        bridge
            .retired_buffers
            .swap(ptr::null_mut(), Ordering::AcqRel),
    ];
    for pointer in maps {
        if !pointer.is_null() {
            // SAFETY: both worker threads have been joined, so these mappings
            // are no longer read.
            drop(unsafe { Box::from_raw(pointer) });
        }
    }
    for pointer in buffers {
        if !pointer.is_null() {
            // SAFETY: the processing thread has exited, so it no longer uses
            // a buffer set stored here.
            drop(unsafe { Box::from_raw(pointer) });
        }
    }
}

struct Loaded {
    _entry: PluginEntry,
    instance: PluginInstance<EchodeHost>,
}

fn load_plugin(
    path: &std::path::Path,
    plugin_id: Option<&str>,
    bridge: Arc<Bridge>,
) -> Result<Loaded, HostError> {
    let entry = load_entry(path)?;
    let factory = entry
        .get_plugin_factory()
        .ok_or_else(|| HostError::Plugin(format!("{} has no plugin factory", path.display())))?;
    let descriptor = factory
        .plugin_descriptors()
        .find(|descriptor| match plugin_id {
            None => true,
            Some(wanted) => descriptor.id().and_then(|id| id.to_str().ok()) == Some(wanted),
        })
        .ok_or_else(|| {
            HostError::Plugin(format!("{} does not contain a plugin", path.display()))
        })?;
    let plugin_id = descriptor
        .id()
        .ok_or_else(|| HostError::Plugin("plugin descriptor has no id".to_string()))?;
    let host_info = HostInfo::new("Echode", "Echode", "https://echode.invalid", "0.0.0")
        .map_err(|error| HostError::Plugin(error.to_string()))?;
    let instance = PluginInstance::<EchodeHost>::new(
        move |_| HostShared {
            bridge: Some(Arc::clone(&bridge)),
        },
        host_main,
        &entry,
        plugin_id,
        &host_info,
    )
    .map_err(|error| HostError::Plugin(error.to_string()))?;
    Ok(Loaded {
        _entry: entry,
        instance,
    })
}

fn activate(
    instance: &mut PluginInstance<EchodeHost>,
    sample_rate: f64,
    max_frames: u32,
) -> Result<StoppedPluginAudioProcessor<EchodeHost>, HostError> {
    instance
        .activate(
            |_, _| (),
            PluginAudioConfiguration {
                sample_rate,
                min_frames_count: 1,
                max_frames_count: max_frames.max(1),
            },
        )
        .map_err(|error| HostError::Plugin(error.to_string()))
}

fn audio_thread(bridge: &Bridge, workgroup: Option<&(String, u32)>) -> Result<(), HostError> {
    if bridge.stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let _membership = match workgroup {
        Some((name, ticks)) => {
            Some(echode_sandbox::join_passed_workgroup(name, *ticks).map_err(HostError::Plugin)?)
        }
        None => None,
    };
    let mut started: Option<StartedPluginAudioProcessor<EchodeHost>> = None;
    if let Err(error) = pump(bridge, &mut started) {
        bridge.stop.store(true, Ordering::Release);
        bridge.bump_control();
        stop_loop(bridge);
        return Err(explain(error));
    }
    if let Err(error) = wait_for_layout(bridge) {
        bridge.stop.store(true, Ordering::Release);
        bridge.bump_control();
        stop_loop(bridge);
        return Err(error);
    }
    let mut seen_alarm = bridge.root.alarm();
    let mut seen_request = 0u64;
    // Allocated once, after the mapping matches the declared ports.
    // Later blocks reuse this storage and do not allocate.
    let shape = published_ports(bridge);
    let (input_counts, output_counts, frames, event_cap) = {
        let region = current_region(bridge);
        (
            shape.map(|shape| shape.inputs.clone()).unwrap_or_default(),
            shape.map(|shape| shape.outputs.clone()).unwrap_or_default(),
            region.layout().max_frames as usize,
            region.layout().input_events as usize,
        )
    };
    let mut buffers = Box::new(PluginBuffers::allocate(
        &input_counts,
        &output_counts,
        frames.max(1),
        event_cap.max(1),
        shape
            .map(|shape| shape.reset_midi.as_slice())
            .unwrap_or_default(),
    ));
    bridge.buffers_live.store(true, Ordering::Release);
    current_region(bridge).mark_ready();
    // `sigaction` is a syscall. Install it before the realtime section.
    echode_sandbox::arm_abort_notice(&bridge.root);
    let result = audio_loop(
        bridge,
        &mut started,
        &mut buffers,
        &mut seen_request,
        &mut seen_alarm,
    );
    if let Some(active) = started.take() {
        let stopped = active.stop_processing();
        bridge.holds_started.store(false, Ordering::Release);
        publish_stopped(&bridge.processor, stopped, false);
    }
    bridge.stop.store(true, Ordering::Release);
    bridge.bump_control();
    stop_loop(bridge);
    result
}

struct PluginBuffers {
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    input_counts: Vec<u32>,
    output_counts: Vec<u32>,
    input_offsets: Vec<usize>,
    output_offsets: Vec<usize>,
    input_ptrs: Vec<*mut f32>,
    output_ptrs: Vec<*mut f32>,
    events: EventBuffer,
    decoded: Vec<Event>,
    reset_midi: Vec<MidiEvent>,
}

impl PluginBuffers {
    /// Sized once from the declared ports. Later blocks reuse this storage.
    fn allocate(
        inputs: &[u32],
        outputs: &[u32],
        frames: usize,
        events: usize,
        reset_midi: &[MidiEvent],
    ) -> Self {
        let input_channels = inputs.iter().map(|count| *count as usize).sum();
        let output_channels = outputs.iter().map(|count| *count as usize).sum();
        Self {
            input_ports: AudioPorts::with_capacity(input_channels, inputs.len().max(1)),
            output_ports: AudioPorts::with_capacity(output_channels, outputs.len().max(1)),
            input: vec![vec![0.0; frames]; input_channels],
            output: vec![vec![0.0; frames]; output_channels],
            input_counts: inputs.to_vec(),
            output_counts: outputs.to_vec(),
            input_offsets: channel_offsets(inputs),
            output_offsets: channel_offsets(outputs),
            input_ptrs: vec![ptr::null_mut(); input_channels],
            output_ptrs: vec![ptr::null_mut(); output_channels],
            // One host lifecycle event in addition to the routed project events.
            events: EventBuffer::with_capacity(
                events.saturating_add(1).saturating_add(reset_midi.len()),
            ),
            decoded: Vec::with_capacity(events),
            reset_midi: reset_midi.to_vec(),
        }
    }

    fn fits(&self, inputs: usize, outputs: usize, frames: usize) -> bool {
        self.input.len() == inputs
            && self.output.len() == outputs
            && self.input.first().is_none_or(|plane| frames <= plane.len())
            && self
                .output
                .first()
                .is_none_or(|plane| frames <= plane.len())
    }
}

enum Step {
    Again,
    Wait,
    Stop,
}

enum RtFail {
    Region(RegionError),
    Process,
    Start,
    Command(u32),
}

impl From<RegionError> for RtFail {
    fn from(error: RegionError) -> Self {
        Self::Region(error)
    }
}

fn explain(error: RtFail) -> HostError {
    match error {
        RtFail::Region(error) => HostError::Region(error),
        RtFail::Process => HostError::Plugin("process refused".to_string()),
        RtFail::Start => HostError::Plugin("plugin refused to start processing".to_string()),
        RtFail::Command(command) => HostError::Region(RegionError::UnknownCommand(command)),
    }
}

fn audio_loop(
    bridge: &Bridge,
    started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>,
    buffers: &mut Box<PluginBuffers>,
    seen_request: &mut u64,
    seen_alarm: &mut u64,
) -> Result<(), HostError> {
    loop {
        if adopt(bridge, Some(buffers)) {
            *seen_request = 0;
            *seen_alarm = bridge.root.alarm();
        }
        if bridge.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        match service_block(bridge, started, buffers, seen_request, seen_alarm) {
            Ok(Step::Again) => continue,
            Ok(Step::Stop) => return Ok(()),
            Ok(Step::Wait) => {
                *seen_alarm = bridge.root.wait_alarm(*seen_alarm)?;
            }
            Err(error) => return Err(explain(error)),
        }
    }
}

/// One realtime step: restart handoff, then at most one block.
/// Allocation and `wait_alarm` stay outside this function.
#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn service_block(
    bridge: &Bridge,
    started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>,
    buffers: &mut PluginBuffers,
    seen_request: &mut u64,
    seen_alarm: &mut u64,
) -> Result<Step, RtFail> {
    let _guard = crate::realtime::ForbidAlloc::enter();
    pump(bridge, started)?;
    flush_params(bridge, started);
    let region = current_region(bridge);
    let published = region.published();
    if published > *seen_request {
        let generation = *seen_request + 1;
        let command = region.block_command(generation);
        if command == CMD_SHUTDOWN {
            region.finish(generation);
            bridge.bump_fan();
            return Ok(Step::Stop);
        }
        if command == CMD_ADOPT_MAPPING {
            region.finish(generation);
            bridge.bump_fan();
            *seen_request = generation;
            return Ok(Step::Again);
        }
        if !matches!(command, CMD_PROCESS | CMD_RESET_PROCESS) {
            return Err(RtFail::Command(command));
        }
        let frames = region.prepare_block(generation)?;
        let inputs = region.input_channels();
        let outputs = region.output_channels();
        let stages = bridge.stages.load(Ordering::Acquire).max(1);
        let fits = buffers.fits(inputs, outputs, frames);
        if bridge.has_plugin.load(Ordering::Acquire) && fits {
            if let Some(active) = started.as_mut() {
                if command == CMD_RESET_PROCESS {
                    active.reset();
                }
                for stage in 0..stages {
                    if stage > 0 {
                        region.feed_output_to_input(generation)?;
                    }
                    if render_plugin(region, generation, active, frames, buffers, &bridge.relay)
                        .is_err()
                    {
                        region.mark_process_refused();
                        return Err(RtFail::Process);
                    }
                }
            } else {
                region.silence_outputs(generation)?;
            }
        } else if bridge.has_plugin.load(Ordering::Acquire) {
            region.silence_outputs(generation)?;
        } else {
            region.copy_inputs_to_outputs(generation)?;
        }
        region.echo_routed(generation, stages);
        region.finish(generation);
        bridge.bump_fan();
        *seen_request = generation;
        return Ok(Step::Again);
    }
    let alarm = bridge.root.alarm();
    if alarm != *seen_alarm {
        *seen_alarm = alarm;
        return Ok(Step::Again);
    }
    Ok(Step::Wait)
}

fn current_region(bridge: &Bridge) -> &MappedRegion {
    let pointer = bridge.current.load(Ordering::Acquire);
    // SAFETY: `current` stays allocated until this thread adopts a replacement
    // or `serve_region` joins this thread and then frees it.
    unsafe { &*pointer }
}

fn handoff_pending(bridge: &Bridge) -> bool {
    !bridge.pending.load(Ordering::Acquire).is_null()
        || !bridge.pending_buffers.load(Ordering::Acquire).is_null()
}

#[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
fn adopt(bridge: &Bridge, buffers: Option<&mut Box<PluginBuffers>>) -> bool {
    let _guard = crate::realtime::ForbidAlloc::enter();
    // The control thread publishes buffers before the mapping. Only this thread
    // removes pending pointers; retired storage is freed by control after release.
    let pending = bridge.pending.load(Ordering::Acquire);
    if pending.is_null() {
        return false;
    }
    let cancel = bridge.cancel_pending.swap(false, Ordering::AcqRel);
    // SAFETY: only this thread can retire this pending mapping.
    if !cancel
        && bridge.buffers_live.load(Ordering::Acquire)
        && unsafe { (*pending).published() } == 0
    {
        return false;
    }
    if !bridge.retired.load(Ordering::Acquire).is_null()
        || !bridge.retired_buffers.load(Ordering::Acquire).is_null()
    {
        if cancel {
            bridge.cancel_pending.store(true, Ordering::Release);
        }
        return false;
    }
    let pending_buffers = bridge.pending_buffers.load(Ordering::Acquire);
    if !cancel && !pending_buffers.is_null() && buffers.is_none() {
        return false;
    }
    bridge.pending.store(ptr::null_mut(), Ordering::Release);
    bridge
        .pending_buffers
        .store(ptr::null_mut(), Ordering::Release);
    if cancel {
        bridge.retired.store(pending, Ordering::Release);
        bridge
            .retired_buffers
            .store(pending_buffers, Ordering::Release);
    } else {
        let old = bridge.current.swap(pending, Ordering::AcqRel);
        bridge.retired.store(old, Ordering::Release);
        if let (Some(buffers), false) = (buffers, pending_buffers.is_null()) {
            // SAFETY: control relinquished this allocation before publishing pending.
            let replacement = unsafe { Box::from_raw(pending_buffers) };
            let old = std::mem::replace(buffers, replacement);
            bridge
                .retired_buffers
                .store(Box::into_raw(old), Ordering::Release);
        }
        if bridge.buffers_live.load(Ordering::Acquire) {
            // SAFETY: current is now exclusively owned by this processing thread.
            unsafe {
                (*pending).mark_ready();
            }
        }
    }
    bridge.switched.fetch_add(1, Ordering::Release);
    bridge.bump_control();
    !cancel
}

fn reclaim_retired(bridge: &Bridge) {
    let old = bridge.retired.swap(ptr::null_mut(), Ordering::AcqRel);
    let buffers = bridge
        .retired_buffers
        .swap(ptr::null_mut(), Ordering::AcqRel);
    // SAFETY: the processing thread relinquished these allocations before release.
    if !old.is_null() {
        drop(unsafe { Box::from_raw(old) });
    }
    if !buffers.is_null() {
        drop(unsafe { Box::from_raw(buffers) });
    }
}

fn pump(
    bridge: &Bridge,
    started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>,
) -> Result<(), RtFail> {
    let want = bridge.restart_request.load(Ordering::Acquire);
    let acked = bridge.restart_ack.load(Ordering::Acquire);
    if want != acked {
        if let Some(active) = started.take() {
            let stopped = active.stop_processing();
            publish_stopped(&bridge.processor, stopped, true);
            bridge.holds_started.store(false, Ordering::Release);
        }
        bridge.restart_ack.store(want, Ordering::Release);
        bridge.bump_control();
    } else if bridge.need_start.swap(false, Ordering::AcqRel)
        && let Some(stopped) = bridge.processor.try_take()
    {
        match stopped.start_processing() {
            Ok(active) => {
                *started = Some(active);
                bridge.holds_started.store(true, Ordering::Release);
            }
            Err(error) => {
                publish_stopped(&bridge.processor, error.into_stopped_processor(), true);
                bridge.stop.store(true, Ordering::Release);
                return Err(RtFail::Start);
            }
        }
    }
    if bridge.want_process.swap(false, Ordering::AcqRel) && started.is_none() {
        if bridge.processor.is_empty() {
            bridge.op.store(OP_ARM, Ordering::Release);
            bridge.signal();
        } else {
            bridge.need_start.store(true, Ordering::Release);
        }
    }
    Ok(())
}

fn control_thread(bridge: &Bridge, mut socket: Option<Socket>) -> Result<(), HostError> {
    if bridge.stop.load(Ordering::Acquire) {
        return Ok(());
    }
    if let Some(socket) = socket.as_mut()
        && bridge.has_plugin.load(Ordering::Acquire)
        && let Some(shape) = published_ports(bridge)
    {
        let message = if let Some(error) = &shape.error {
            ControlMessage::PortsRejected(error.clone())
        } else {
            ControlMessage::Ports {
                inputs: shape.inputs.clone(),
                outputs: shape.outputs.clone(),
                tail_samples: bridge.tail_samples.load(Ordering::Acquire),
            }
        };
        if socket.send(&message).is_err() {
            bridge.stop.store(true, Ordering::Release);
            bridge.bump_audio();
            stop_loop(bridge);
            return Ok(());
        }
        if shape.error.is_some() {
            bridge.stop.store(true, Ordering::Release);
            bridge.bump_audio();
            stop_loop(bridge);
            return Ok(());
        }
    }
    if std::env::var("ECHODE_TEST_FAULT").as_deref() == Ok("restart-idle") {
        thread::sleep(std::time::Duration::from_millis(30));
        bridge.restart_request.fetch_add(1, Ordering::Release);
        bridge.bump_audio();
    }
    if std::env::var("ECHODE_TEST_FAULT").as_deref() == Ok("dirty")
        && let Some(socket) = &mut socket
    {
        let _ = socket.send(&ControlMessage::Dirty);
    }
    let mut seen = 0u64;
    let mut restarted = 0u64;
    loop {
        if bridge.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let want = bridge.restart_request.load(Ordering::Acquire);
        if want != restarted {
            while bridge.restart_ack.load(Ordering::Acquire) < want
                || bridge.holds_started.load(Ordering::Acquire)
            {
                if bridge.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                seen = pause(bridge, socket.as_ref(), seen);
            }
            if bridge.has_plugin.load(Ordering::Acquire) {
                bridge.restart_for_main.store(want, Ordering::Release);
                bridge.op.store(OP_RESTART, Ordering::Release);
                bridge.signal();
                while bridge.main_done.load(Ordering::Acquire) < want {
                    if bridge.stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    seen = pause(bridge, socket.as_ref(), seen);
                }
            } else {
                emit_line("restart-observed");
            }
            if let Some(socket) = &mut socket {
                let _ = socket.send(&ControlMessage::PluginRestarted { latency_samples: 0 });
            }
            restarted = want;
            continue;
        }
        if bridge.plugin_dirty.swap(false, Ordering::AcqRel)
            && let Some(socket) = &mut socket
        {
            let _ = socket.send(&ControlMessage::Dirty);
        }
        if bridge.process_note.swap(false, Ordering::AcqRel)
            && let Some(socket) = &mut socket
        {
            let _ = socket.send(&ControlMessage::RequestProcess);
        }
        if let Some(socket) = &mut socket {
            send_relay(bridge, socket);
            let commands = std::mem::take(
                &mut *bridge
                    .gui_commands
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
            for command in commands {
                let _ = socket.send(&ControlMessage::GuiCommand(command));
            }
            if bridge.gui_closed.swap(false, Ordering::AcqRel) {
                let _ = socket.send(&ControlMessage::GuiClosed);
            }
        }
        if let Some(socket) = &mut socket {
            match socket.try_recv() {
                Ok(Some(message)) => {
                    if handle_control(bridge, socket, message).is_err() {
                        bridge.stop.store(true, Ordering::Release);
                        bridge.bump_audio();
                        stop_loop(bridge);
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    bridge.stop.store(true, Ordering::Release);
                    bridge.bump_audio();
                    stop_loop(bridge);
                    return Ok(());
                }
            }
            seen = pause(bridge, Some(socket), seen);
        } else {
            seen = wait_control(bridge, seen);
        }
    }
}

fn handle_control(
    bridge: &Bridge,
    socket: &mut Socket,
    message: ControlMessage,
) -> Result<(), HostError> {
    match message {
        ControlMessage::SaveRequest => {
            let reply = match call_plugin_state(bridge, None) {
                Ok(bytes) => ControlMessage::State(bytes),
                Err(error) => ControlMessage::StateRejected(error),
            };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::Restore(bytes) => {
            let reply = match call_plugin_state(bridge, Some(bytes)) {
                Ok(_) => ControlMessage::Restored,
                Err(error) => ControlMessage::StateRejected(error),
            };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::ParamsRequest => {
            let reply = match read_params(bridge) {
                Ok(infos) => ControlMessage::Params(infos),
                Err(error) => ControlMessage::ParamsRejected(error),
            };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::SetParams(changes) => {
            let reply = match apply_params(bridge, &changes) {
                Ok(values) => ControlMessage::ParamsApplied(values),
                Err(error) => ControlMessage::ParamsRejected(error),
            };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::GuiShow(title) => {
            let reply = match call_plugin_operation_within(
                bridge,
                GUI_SHOW,
                title.into_bytes(),
                WINDOW_WAIT,
            )
            .and_then(|bytes| decode_report(&bytes))
            {
                Ok(report) => ControlMessage::GuiShown(report),
                Err(error) => ControlMessage::GuiRejected(error),
            };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::GuiClose => {
            let reply =
                match call_plugin_operation_within(bridge, GUI_CLOSE, Vec::new(), WINDOW_WAIT) {
                    Ok(_) => ControlMessage::GuiClosedReply,
                    Err(error) => ControlMessage::GuiRejected(error),
                };
            socket
                .send(&reply)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::SetChain(stages) => {
            if stages == 0 {
                socket
                    .send(&ControlMessage::ChainRejected)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
            } else {
                bridge.stages.store(stages, Ordering::Release);
                socket
                    .send(&ControlMessage::ChainReady)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
            }
        }
        ControlMessage::CancelRemap => {
            if bridge.pending.load(Ordering::Acquire).is_null() {
                socket
                    .send(&ControlMessage::RemapFailed)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
                return Ok(());
            }
            let target = bridge.switched.load(Ordering::Acquire) + 1;
            bridge.cancel_pending.store(true, Ordering::Release);
            bridge.bump_audio();
            while bridge.switched.load(Ordering::Acquire) < target {
                if bridge.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                thread::sleep(std::time::Duration::from_millis(1));
            }
            reclaim_retired(bridge);
            socket
                .send(&ControlMessage::RemapCancelled)
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::Remap { layout, fd } => match MappedRegion::from_fd(fd) {
            Ok(region)
                if region.layout() == layout
                    && bridge.pending.load(Ordering::Acquire).is_null() =>
            {
                if bridge.has_plugin.load(Ordering::Acquire)
                    && call_plugin_operation(
                        bridge,
                        PREPARE_CAPACITY,
                        layout.input_events.to_le_bytes().to_vec(),
                    )
                    .is_err()
                {
                    socket
                        .send(&ControlMessage::RemapFailed)
                        .map_err(|error| HostError::Plugin(error.to_string()))?;
                    return Ok(());
                }
                let buffers = if bridge.buffers_live.load(Ordering::Acquire) {
                    Box::into_raw(Box::new(buffers_for(bridge, &region)))
                } else {
                    ptr::null_mut()
                };
                reclaim_retired(bridge);
                let pointer = Box::into_raw(Box::new(Arc::new(region)));
                bridge.pending_buffers.store(buffers, Ordering::Release);
                bridge.pending.store(pointer, Ordering::Release);
                bridge.bump_audio();
                socket
                    .send(&ControlMessage::Remapped)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
            }
            Ok(region) => {
                drop(region);
                socket
                    .send(&ControlMessage::RemapFailed)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
            }
            Err(_) => {
                socket
                    .send(&ControlMessage::RemapFailed)
                    .map_err(|error| HostError::Plugin(error.to_string()))?;
            }
        },
        _ => {}
    }
    Ok(())
}

fn reject_launch(socket: Option<&mut Socket>, reason: &str) {
    if let Some(socket) = socket {
        let _ = socket.send(&ControlMessage::PortsRejected(reason.to_string()));
    }
}

fn pause(bridge: &Bridge, socket: Option<&Socket>, seen: u64) -> u64 {
    if let Some(socket) = socket {
        let _ = socket.wait_readable(std::time::Duration::from_millis(20));
        bridge.control_alarm.load(Ordering::Acquire)
    } else {
        wait_control(bridge, seen)
    }
}

fn wait_control(bridge: &Bridge, seen: u64) -> u64 {
    echode_sandbox::wait_until_ne(&bridge.control_alarm, seen, SyncFlags::THREAD)
        .unwrap_or(seen.wrapping_add(1))
}

fn render_plugin(
    region: &MappedRegion,
    generation: u64,
    started: &mut StartedPluginAudioProcessor<EchodeHost>,
    frames: usize,
    buffers: &mut PluginBuffers,
    relay: &RelayRing,
) -> Result<(), ()> {
    for (channel, plane) in buffers.input.iter_mut().enumerate() {
        unsafe {
            ptr::copy_nonoverlapping(
                region.input_ptr(generation, channel),
                plane.as_mut_ptr(),
                frames,
            );
        }
    }
    refresh_ptrs(&mut buffers.input, &mut buffers.input_ptrs);
    refresh_ptrs(&mut buffers.output, &mut buffers.output_ptrs);
    let input_ptr = buffers.input_ptrs.as_ptr();
    let output_ptr = buffers.output_ptrs.as_ptr();
    // Standard iterators, so this builds no port or channel vector per block.
    let input_ports = buffers
        .input_counts
        .iter()
        .zip(buffers.input_offsets.iter())
        .map(move |(&count, &start)| port_inputs(input_ptr, start, count as usize, frames));
    let output_ports = buffers
        .output_counts
        .iter()
        .zip(buffers.output_offsets.iter())
        .map(move |(&count, &start)| port_outputs(output_ptr, start, count as usize, frames));
    let input_audio = buffers.input_ports.with_input_buffers(input_ports);
    let mut output_audio = buffers.output_ports.with_output_buffers(output_ports);
    buffers.events.clear();
    if region.block_command(generation) == CMD_RESET_PROCESS {
        buffers
            .events
            .push(&NoteChokeEvent::new(0, Pckn::match_all()));
        for event in &buffers.reset_midi {
            buffers.events.push(event);
        }
    }
    region.copy_input_events(generation, &mut buffers.decoded);
    write_input(&buffers.decoded, &mut buffers.events);
    let input_events = InputEvents::from_buffer(&buffers.events);
    // Values and gestures the plugin reports go to the ring; nothing allocates.
    let mut relay = RelayOutput(relay);
    let mut output_events = OutputEvents::from_buffer(&mut relay);
    // The error string would allocate. The parent reads the refusal flag.
    started
        .process(
            &input_audio,
            &mut output_audio,
            &input_events,
            &mut output_events,
            None,
            None,
        )
        .map_err(|_| ())?;
    for (channel, plane) in buffers.output.iter().enumerate() {
        unsafe {
            ptr::copy_nonoverlapping(
                plane.as_ptr(),
                region.output_ptr(generation, channel),
                frames,
            );
        }
    }
    Ok(())
}

fn channel_offsets(counts: &[u32]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(counts.len());
    let mut total = 0usize;
    for count in counts {
        offsets.push(total);
        total = total.saturating_add(*count as usize);
    }
    offsets
}

fn port_inputs<'a>(
    ptrs: *const *mut f32,
    start: usize,
    count: usize,
    frames: usize,
) -> AudioPortBuffer<
    impl ExactSizeIterator<Item = InputChannel<'a, f32>>,
    std::array::IntoIter<InputChannel<'static, f64>, 0>,
> {
    let channels = (0..count).map(move |index| {
        // SAFETY: `ptrs` addresses the buffer set for this block, and `start + index`
        // is one of its channels. The slice lives only while clack copies the pointer.
        let pointer = unsafe { *ptrs.add(start + index) };
        InputChannel::variable(unsafe { std::slice::from_raw_parts_mut(pointer, frames) })
    });
    AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_input_only(channels),
    }
}

fn port_outputs<'a>(
    ptrs: *const *mut f32,
    start: usize,
    count: usize,
    frames: usize,
) -> AudioPortBuffer<
    impl ExactSizeIterator<Item = &'a mut [f32]>,
    std::array::IntoIter<&'static mut [f64], 0>,
> {
    let channels = (0..count).map(move |index| {
        // SAFETY: same region as `port_inputs`. Clack copies the pointer before returning.
        let pointer = unsafe { *ptrs.add(start + index) };
        unsafe { std::slice::from_raw_parts_mut(pointer, frames) }
    });
    AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_output_only(channels),
    }
}

fn buffers_for(bridge: &Bridge, region: &MappedRegion) -> PluginBuffers {
    let shape = published_ports(bridge);
    let inputs = shape.map(|shape| shape.inputs.clone()).unwrap_or_default();
    let outputs = shape.map(|shape| shape.outputs.clone()).unwrap_or_default();
    let layout = region.layout();
    PluginBuffers::allocate(
        &inputs,
        &outputs,
        layout.max_frames.max(1) as usize,
        layout.input_events.max(1) as usize,
        shape
            .map(|shape| shape.reset_midi.as_slice())
            .unwrap_or_default(),
    )
}

fn refresh_ptrs(planes: &mut [Vec<f32>], ptrs: &mut [*mut f32]) {
    for (plane, slot) in planes.iter_mut().zip(ptrs.iter_mut()) {
        *slot = plane.as_mut_ptr();
    }
}

struct PortShape {
    inputs: Vec<u32>,
    outputs: Vec<u32>,
    reset_midi: Vec<MidiEvent>,
    error: Option<String>,
}

fn published_ports(bridge: &Bridge) -> Option<&PortShape> {
    let pointer = bridge.ports.load(Ordering::Acquire);
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { &*pointer })
    }
}

fn wait_for_layout(bridge: &Bridge) -> Result<(), HostError> {
    loop {
        if bridge.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if !adopt(bridge, None) && handoff_pending(bridge) {
            std::thread::yield_now();
            continue;
        }
        if !bridge.has_plugin.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(shape) = published_ports(bridge) else {
            return Err(HostError::Plugin(
                "plugin ports were not published".to_string(),
            ));
        };
        if shape.error.is_some() {
            let alarm = bridge.root.alarm();
            let _ = bridge.root.wait_alarm(alarm);
            continue;
        }
        let region = current_region(bridge);
        let inputs = sum_ports(&shape.inputs);
        let outputs = sum_ports(&shape.outputs);
        if region.input_channels() == inputs && region.output_channels() == outputs {
            return Ok(());
        }
        let alarm = bridge.root.alarm();
        let _ = bridge.root.wait_alarm(alarm);
    }
}

fn sum_ports(counts: &[u32]) -> usize {
    counts
        .iter()
        .fold(0usize, |total, count| total.saturating_add(*count as usize))
}

/// Asks the main thread to save, or to load `bytes`. `None` saves.
///
/// The audio thread does not call this. A missing extension or a refused
/// stream is an error, not an empty success.
fn call_plugin_state(bridge: &Bridge, load: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    if !bridge.has_plugin.load(Ordering::Acquire) {
        return Err("no plugin".to_string());
    }
    let kind = if load.is_some() {
        STATE_LOAD
    } else {
        STATE_SAVE
    };
    call_plugin_operation(bridge, kind, load.unwrap_or_default())
}

fn call_plugin_operation(bridge: &Bridge, kind: u8, input: Vec<u8>) -> Result<Vec<u8>, String> {
    call_plugin_operation_within(bridge, kind, input, STATE_WAIT)
}

fn call_plugin_operation_within(
    bridge: &Bridge,
    kind: u8,
    input: Vec<u8>,
    wait: std::time::Duration,
) -> Result<Vec<u8>, String> {
    if !bridge.has_plugin.load(Ordering::Acquire) {
        return Err("no plugin".to_string());
    }
    {
        let mut bytes = bridge
            .state_bytes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *bytes = input;
    }
    *bridge
        .state_error
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    let generation = bridge.state_gen.fetch_add(1, Ordering::AcqRel) + 1;
    bridge.state_kind.store(kind, Ordering::Release);
    bridge.signal();
    let started = std::time::Instant::now();
    while bridge.state_done.load(Ordering::Acquire) < generation {
        if bridge.stop.load(Ordering::Acquire) {
            return Err("plugin stopped before state finished".to_string());
        }
        if started.elapsed() > wait {
            return Err(if kind == GUI_SHOW || kind == GUI_CLOSE {
                "plugin window timed out".to_string()
            } else {
                "plugin state timed out".to_string()
            });
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    if let Some(error) = bridge
        .state_error
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
    {
        return Err(error);
    }
    Ok(bridge
        .state_bytes
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone())
}

fn perform_plugin_state(bridge: &Bridge, kind: u8) -> Result<Vec<u8>, String> {
    INSTANCE.with(|cell| {
        let instance = cell.get();
        if instance.is_null() {
            return Err("plugin is not loaded".to_string());
        }
        let instance = unsafe { &*instance };
        if kind == STATE_SAVE {
            return save_plugin_state(instance);
        }
        let bytes = bridge
            .state_bytes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        match kind {
            PREPARE_CAPACITY => {
                let capacity = u32::from_le_bytes(
                    bytes
                        .as_slice()
                        .try_into()
                        .map_err(|_| "invalid capacity")?,
                );
                prepare_event_capacity(instance, capacity)?;
            }
            PARAM_LIST | PARAM_READ | PARAM_RELAY => {
                let read = read_on_main(bridge, instance, kind, &bytes);
                *bridge
                    .param_read
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = read;
            }
            GUI_SHOW => {
                let title = String::from_utf8_lossy(&bytes);
                let report = gui::show(instance.raw_instance(), &title)?;
                return Ok(encode_report(report));
            }
            GUI_CLOSE => gui::close(instance.raw_instance()),
            _ => load_plugin_state(instance, &bytes)?,
        }
        Ok(bytes)
    })
}

/// Changes published together by the control thread and flushed together.
struct ParamBatch {
    generation: u64,
    events: EventBuffer,
    /// The next older batch on the retired list.
    next: *mut ParamBatch,
}

enum ParamRead {
    Infos(Vec<ParamInfo>),
    Values(Vec<ParamValue>),
}

fn params_extension(instance: &PluginInstance<EchodeHost>) -> *mut clap_plugin_params {
    let plugin = instance.raw_instance();
    let Some(get) = plugin.get_extension else {
        return ptr::null_mut();
    };
    // SAFETY: standard extension id, queried on the main thread after creation.
    // The table stays valid for the plugin's lifetime.
    unsafe { get(plugin, CLAP_EXT_PARAMS.as_ptr()) }
        .cast::<clap_plugin_params>()
        .cast_mut()
}

/// Applies queued parameter values between blocks, on the processing thread.
/// CLAP allows `flush` there while the plugin is active, and never during
/// `process`. The control thread allocated the batch; this only moves pointers.
/// A plugin's own `request_flush` runs `flush` with no input, so changes made
/// in its window reach the relay while the transport is stopped.
fn flush_params(bridge: &Bridge, started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>) {
    let Some(active) = started.as_mut() else {
        return;
    };
    let requested = bridge.flush_request.swap(false, Ordering::AcqRel);
    let batch = if bridge.param_pending.load(Ordering::Acquire).is_null() {
        ptr::null_mut()
    } else {
        bridge.param_pending.swap(ptr::null_mut(), Ordering::AcqRel)
    };
    if batch.is_null() && !requested {
        return;
    }
    let params = bridge.params.load(Ordering::Acquire);
    let mut relay = RelayOutput(&bridge.relay);
    // SAFETY: the control thread does not touch a published batch again until
    // it is on the retired list, and the extension table outlives the plugin.
    unsafe {
        if let Some(flush) = params.as_ref().and_then(|params| params.flush) {
            let input = if batch.is_null() {
                InputEvents::empty()
            } else {
                InputEvents::from_buffer(&(*batch).events)
            };
            let mut output = OutputEvents::from_buffer(&mut relay);
            flush(
                active.plugin_handle().as_raw(),
                input.as_raw(),
                output.as_raw(),
            );
        }
    }
    if batch.is_null() {
        bridge.bump_control();
        return;
    }
    // SAFETY: this thread took the batch from `param_pending` above.
    let generation = unsafe { (*batch).generation };
    let mut head = bridge.param_retired.load(Ordering::Acquire);
    loop {
        // SAFETY: this thread owns the batch until the exchange publishes it.
        unsafe { (*batch).next = head };
        match bridge.param_retired.compare_exchange_weak(
            head,
            batch,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(current) => head = current,
        }
    }
    bridge.param_flushed.store(generation, Ordering::Release);
    bridge.bump_control();
}

/// Frees every flushed batch. Only the control thread and teardown call this.
fn reclaim_param_batches(bridge: &Bridge) {
    let mut batch = bridge.param_retired.swap(ptr::null_mut(), Ordering::AcqRel);
    while !batch.is_null() {
        // SAFETY: swapping the list moved every retired batch to this thread.
        let owned = unsafe { Box::from_raw(batch) };
        batch = owned.next;
    }
}

fn read_params(bridge: &Bridge) -> Result<Vec<ParamInfo>, String> {
    call_plugin_operation(bridge, PARAM_LIST, Vec::new())?;
    match take_param_read(bridge) {
        ParamRead::Infos(infos) => Ok(infos),
        ParamRead::Values(_) => Err("parameter read returned values".to_string()),
    }
}

fn take_param_read(bridge: &Bridge) -> ParamRead {
    std::mem::replace(
        &mut *bridge
            .param_read
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        ParamRead::Values(Vec::new()),
    )
}

/// Publishes `changes` for the processing thread, waits until they were
/// flushed, then reads those values back on the main thread.
fn apply_params(bridge: &Bridge, changes: &[ParamChange]) -> Result<Vec<ParamValue>, String> {
    if !bridge.has_plugin.load(Ordering::Acquire) {
        return Err("no plugin".to_string());
    }
    let params = bridge.params.load(Ordering::Acquire);
    // SAFETY: the extension table lives as long as the plugin instance.
    if unsafe { params.as_ref() }
        .and_then(|params| params.flush)
        .is_none()
    {
        return Err("plugin does not accept parameter changes".to_string());
    }
    let mut events = EventBuffer::with_capacity(changes.len());
    let mut ids: Vec<u32> = Vec::with_capacity(changes.len());
    for change in changes {
        let id = ClapId::from_raw(change.id)
            .filter(|_| change.value.is_finite())
            .ok_or_else(|| "invalid parameter change".to_string())?;
        events.push(&ParamValueEvent::new(
            0,
            id,
            Pckn::match_all(),
            change.value,
        ));
        if !ids.contains(&change.id) {
            ids.push(change.id);
        }
    }
    reclaim_param_batches(bridge);
    let generation = bridge.param_generation.fetch_add(1, Ordering::AcqRel) + 1;
    let batch = Box::into_raw(Box::new(ParamBatch {
        generation,
        events,
        next: ptr::null_mut(),
    }));
    let stale = bridge.param_pending.swap(batch, Ordering::AcqRel);
    if !stale.is_null() {
        // The processing thread never took it, so this thread still owns it.
        drop(unsafe { Box::from_raw(stale) });
    }
    bridge.bump_audio();
    let started = std::time::Instant::now();
    while bridge.param_flushed.load(Ordering::Acquire) < generation {
        if bridge.stop.load(Ordering::Acquire) {
            return Err("plugin stopped before parameters were applied".to_string());
        }
        if started.elapsed() > std::time::Duration::from_secs(2) {
            let unclaimed = bridge.param_pending.swap(ptr::null_mut(), Ordering::AcqRel);
            if !unclaimed.is_null() {
                drop(unsafe { Box::from_raw(unclaimed) });
            }
            return Err("parameter changes timed out".to_string());
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    reclaim_param_batches(bridge);
    let bytes = ids.iter().flat_map(|id| id.to_le_bytes()).collect();
    call_plugin_operation(bridge, PARAM_READ, bytes)?;
    match take_param_read(bridge) {
        ParamRead::Values(values) => Ok(values),
        ParamRead::Infos(_) => Err("parameter read returned descriptions".to_string()),
    }
}

/// `clap.params` reads are main-thread calls. A plugin without the extension
/// has no parameters, which is an empty result rather than an error.
fn read_on_main(
    bridge: &Bridge,
    instance: &PluginInstance<EchodeHost>,
    kind: u8,
    input: &[u8],
) -> ParamRead {
    let plugin = instance.raw_instance();
    let params = bridge.params.load(Ordering::Acquire);
    // SAFETY: the extension table lives as long as the plugin instance.
    let Some(params) = (unsafe { params.as_ref() }) else {
        return if kind == PARAM_LIST {
            ParamRead::Infos(Vec::new())
        } else {
            ParamRead::Values(Vec::new())
        };
    };
    if kind == PARAM_LIST {
        return ParamRead::Infos(param_infos(plugin, params));
    }
    // A relay leaves out ids the plugin does not describe and read-only
    // parameters, which report output such as meters rather than an edit.
    let flags = (kind == PARAM_RELAY).then(|| {
        PARAM_FLAGS.with(|cache| {
            cache
                .borrow_mut()
                .get_or_insert_with(|| param_flags(plugin, params))
                .clone()
        })
    });
    ParamRead::Values(
        input
            .chunks_exact(4)
            .filter_map(|chunk| {
                let id = u32::from_le_bytes(chunk.try_into().ok()?);
                if let Some(flags) = &flags
                    && flags
                        .get(&id)
                        .is_none_or(|bits| bits & CLAP_PARAM_IS_READONLY != 0)
                {
                    return None;
                }
                let value = param_value(plugin, params, id)?;
                Some(ParamValue {
                    id,
                    value,
                    text: param_text(plugin, params, id, value),
                })
            })
            .collect(),
    )
}

/// Every described id and its flags, for filtering relays.
fn param_flags(plugin: &clap_plugin, params: &clap_plugin_params) -> HashMap<u32, u32> {
    let (Some(count), Some(get_info)) = (params.count, params.get_info) else {
        return HashMap::new();
    };
    // SAFETY: called on the plugin main thread, as `clap.params` requires.
    let total = unsafe { count(plugin) };
    let mut flags = HashMap::with_capacity(total as usize);
    for index in 0..total {
        let mut raw = MaybeUninit::<clap_param_info>::zeroed();
        // SAFETY: main thread; `raw` is writable storage for one description.
        if unsafe { get_info(plugin, index, raw.as_mut_ptr()) } {
            // SAFETY: the plugin filled the description it accepted.
            let info = unsafe { raw.assume_init() };
            flags.insert(info.id, info.flags);
        }
    }
    flags
}

/// Sends what the plugin reported on its own since the last call: gestures
/// and value events in order, with the values read back on the main thread.
fn send_relay(bridge: &Bridge, socket: &mut Socket) {
    if !bridge.relay.pending() && !bridge.rescan_request.load(Ordering::Acquire) {
        return;
    }
    let rescan = bridge.rescan_request.swap(false, Ordering::AcqRel);
    let mut events: Vec<ParamEvent> = Vec::new();
    let lost = bridge.relay.drain(&mut events);
    let mut ids: Vec<u32> = Vec::new();
    for event in &events {
        if !ids.contains(&event.id) {
            ids.push(event.id);
        }
    }
    let values = if ids.is_empty() {
        Vec::new()
    } else {
        let bytes = ids.iter().flat_map(|id| id.to_le_bytes()).collect();
        match call_plugin_operation(bridge, PARAM_RELAY, bytes).map(|_| take_param_read(bridge)) {
            Ok(ParamRead::Values(values)) => values,
            // The values could not be read; the parent reads the list again.
            _ => {
                let _ = socket.send(&ControlMessage::ParamRelay(ParamRelay {
                    lost: true,
                    rescan: true,
                    ..ParamRelay::default()
                }));
                return;
            }
        }
    };
    events.retain(|event| values.iter().any(|value| value.id == event.id));
    let relay = ParamRelay {
        events,
        values,
        rescan,
        lost,
    };
    if !relay.is_empty() {
        let _ = socket.send(&ControlMessage::ParamRelay(relay));
    }
}

fn encode_report(report: GuiReport) -> Vec<u8> {
    vec![
        u8::from(report.created),
        u8::from(report.visible),
        u8::from(report.key),
    ]
}

fn decode_report(bytes: &[u8]) -> Result<GuiReport, String> {
    let [created, visible, key] = bytes else {
        return Err("window report is the wrong size".to_string());
    };
    Ok(GuiReport {
        created: *created != 0,
        visible: *visible != 0,
        key: *key != 0,
    })
}

/// Applies the plugin's window requests and follows the active app, once per
/// turn of the main run loop. A window the user closed is reported once.
fn tick_window(bridge: &Bridge) {
    INSTANCE.with(|cell| {
        let instance = cell.get();
        if instance.is_null() {
            return;
        }
        // SAFETY: the instance stays loaded while the main loop runs.
        let plugin = unsafe { (*instance).raw_instance() };
        if gui::tick(plugin, &bridge.gui) {
            bridge.gui_closed.store(true, Ordering::Release);
            bridge.bump_control();
        }
    });
}

/// Descriptions the plugin reports now. A refused index, CLAP's invalid id, or
/// a non-finite or inverted range is skipped rather than repaired.
fn param_infos(plugin: &clap_plugin, params: &clap_plugin_params) -> Vec<ParamInfo> {
    let (Some(count), Some(get_info)) = (params.count, params.get_info) else {
        return Vec::new();
    };
    // SAFETY: called on the plugin main thread, as `clap.params` requires.
    let total = unsafe { count(plugin) };
    let mut infos = Vec::new();
    for index in 0..total {
        let mut raw = MaybeUninit::<clap_param_info>::zeroed();
        // SAFETY: the index is below the count the plugin just returned.
        if !unsafe { get_info(plugin, index, raw.as_mut_ptr()) } {
            continue;
        }
        // SAFETY: all-zero is a valid `clap_param_info`, and the plugin filled it.
        let raw = unsafe { raw.assume_init() };
        if raw.id == u32::MAX {
            continue;
        }
        let value = param_value(plugin, params, raw.id).unwrap_or(f64::NAN);
        let info = ParamInfo {
            id: raw.id,
            flags: raw.flags,
            name: c_text(&raw.name),
            module: c_text(&raw.module),
            min: raw.min_value,
            max: raw.max_value,
            default: raw.default_value,
            value,
            text: if value.is_finite() {
                param_text(plugin, params, raw.id, value)
            } else {
                String::new()
            },
        };
        if info.usable() {
            infos.push(info);
        }
    }
    infos
}

fn param_value(plugin: &clap_plugin, params: &clap_plugin_params, id: u32) -> Option<f64> {
    let get_value = params.get_value?;
    let mut value = 0.0;
    // SAFETY: main thread, and `value` is writable for the call.
    unsafe { get_value(plugin, id, &mut value) }
        .then_some(value)
        .filter(|value| value.is_finite())
}

fn param_text(plugin: &clap_plugin, params: &clap_plugin_params, id: u32, value: f64) -> String {
    let Some(to_text) = params.value_to_text else {
        return String::new();
    };
    // Display text for one value. The plugin truncates to this buffer; it is
    // not a limit on parameters or their count.
    let mut buffer = [0 as c_char; 256];
    // SAFETY: main thread; the capacity matches the buffer passed.
    if !unsafe { to_text(plugin, id, value, buffer.as_mut_ptr(), buffer.len() as u32) } {
        return String::new();
    }
    c_text(&buffer)
}

/// Text up to the first NUL, or the whole array when the plugin left none.
fn c_text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|ch| **ch != 0)
        .map(|ch| *ch as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn query_tail(instance: &PluginInstance<EchodeHost>) -> u32 {
    use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
    let plugin = instance.raw_instance();
    let Some(get) = plugin.get_extension else {
        return 0;
    };
    // SAFETY: standard CLAP extension, queried on the main thread after activation.
    let raw = unsafe { get(plugin, CLAP_EXT_TAIL.as_ptr()) };
    if raw.is_null() {
        return 0;
    }
    let extension = unsafe { &*raw.cast::<clap_plugin_tail>() };
    extension.get.map(|get| unsafe { get(plugin) }).unwrap_or(0)
}

/// Optional vendor extension; every plugin is prepared through the same path.
fn prepare_event_capacity(
    instance: &PluginInstance<EchodeHost>,
    events: u32,
) -> Result<(), String> {
    use echode_realtime::plugin_capacity::{PLUGIN_EVENT_CAPACITY, PluginEventCapacity};
    let plugin = instance.raw_instance();
    let Some(get) = plugin.get_extension else {
        return Ok(());
    };
    // SAFETY: the extension is queried and called on the plugin main thread.
    let raw = unsafe { get(plugin, PLUGIN_EVENT_CAPACITY.as_ptr()) };
    if raw.is_null() {
        return Ok(());
    }
    let extension = unsafe { &*raw.cast::<PluginEventCapacity>() };
    if unsafe { (extension.prepare)((plugin as *const clap_plugin).cast(), events) } {
        Ok(())
    } else {
        Err("plugin refused event capacity preparation".into())
    }
}

fn state_extension<'a>(plugin: *const clap_plugin) -> Result<&'a clap_plugin_state, String> {
    if plugin.is_null() {
        return Err("plugin is not loaded".to_string());
    }
    let get_extension = unsafe { (*plugin).get_extension }
        .ok_or_else(|| "plugin does not declare state".to_string())?;
    let extension = unsafe { get_extension(plugin, CLAP_EXT_STATE.as_ptr()) };
    if extension.is_null() {
        return Err("plugin does not declare state".to_string());
    }
    Ok(unsafe { &*(extension as *const clap_plugin_state) })
}

fn save_plugin_state(instance: &PluginInstance<EchodeHost>) -> Result<Vec<u8>, String> {
    let plugin = instance.raw_instance();
    let save = state_extension(plugin)?
        .save
        .ok_or_else(|| "plugin state has no save".to_string())?;
    let mut bytes = Vec::new();
    let stream = clap_ostream {
        ctx: (&mut bytes as *mut Vec<u8>).cast::<c_void>(),
        write: Some(state_write),
    };
    let saved = unsafe { save(plugin, &stream) };
    if !saved {
        return Err("plugin refused to save state".to_string());
    }
    Ok(bytes)
}

fn load_plugin_state(instance: &PluginInstance<EchodeHost>, bytes: &[u8]) -> Result<(), String> {
    let plugin = instance.raw_instance();
    let load = state_extension(plugin)?
        .load
        .ok_or_else(|| "plugin state has no load".to_string())?;
    let mut cursor = ReadCursor {
        bytes: bytes.as_ptr(),
        len: bytes.len(),
        pos: 0,
    };
    let stream = clap_istream {
        ctx: (&mut cursor as *mut ReadCursor).cast::<c_void>(),
        read: Some(state_read),
    };
    let loaded = unsafe { load(plugin, &stream) };
    if !loaded {
        return Err("plugin refused to load state".to_string());
    }
    Ok(())
}

struct ReadCursor {
    bytes: *const u8,
    len: usize,
    pos: usize,
}

unsafe extern "C" fn state_write(
    stream: *const clap_ostream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || (buffer.is_null() && size != 0) {
        return -1;
    }
    let Ok(count) = usize::try_from(size) else {
        return -1;
    };
    let ctx = unsafe { &mut *(*stream).ctx.cast::<Vec<u8>>() };
    if count > 0 {
        let slice = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), count) };
        ctx.extend_from_slice(slice);
    }
    size as i64
}

unsafe extern "C" fn state_read(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || (buffer.is_null() && size != 0) {
        return -1;
    }
    let Ok(count) = usize::try_from(size) else {
        return -1;
    };
    let ctx = unsafe { &mut *(*stream).ctx.cast::<ReadCursor>() };
    let available = ctx.len.saturating_sub(ctx.pos);
    let take = count.min(available);
    if take > 0 {
        unsafe {
            ptr::copy_nonoverlapping(ctx.bytes.add(ctx.pos), buffer.cast::<u8>(), take);
        }
        ctx.pos += take;
    }
    take as i64
}

/// Reads `clap.audio-ports` while the plugin is inactive.
fn query_audio_ports(instance: &PluginInstance<EchodeHost>) -> Result<PortShape, String> {
    let plugin = instance.raw_instance();
    let get_extension = plugin
        .get_extension
        .ok_or_else(|| "plugin does not declare audio ports".to_string())?;
    // SAFETY: `get_extension` is the plugin's own function. The id is a static
    // C string. The plugin is inactive and this runs on the main thread.
    let extension = unsafe { get_extension(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr()) };
    if extension.is_null() {
        return Err("plugin does not declare audio ports".to_string());
    }
    let ports = unsafe { &*(extension as *const clap_plugin_audio_ports) };
    let count = ports
        .count
        .ok_or_else(|| "plugin audio ports have no count".to_string())?;
    let get = ports
        .get
        .ok_or_else(|| "plugin audio ports have no description".to_string())?;
    let inputs = read_port_list(plugin, count, get, true)?;
    let outputs = read_port_list(plugin, count, get, false)?;
    if inputs.is_empty() && outputs.is_empty() {
        return Err("plugin declares no audio ports".to_string());
    }
    Ok(PortShape {
        inputs,
        outputs,
        reset_midi: query_midi_resets(plugin)?,
        error: None,
    })
}

fn query_midi_resets(plugin: &clap_plugin) -> Result<Vec<MidiEvent>, String> {
    let mut result = Vec::new();
    let Some(get_extension) = plugin.get_extension else {
        return Ok(result);
    };
    // SAFETY: main thread, inactive plugin, typed public CLAP extension.
    let raw = unsafe { get_extension(plugin, CLAP_EXT_NOTE_PORTS.as_ptr()) };
    if raw.is_null() {
        return Ok(result);
    }
    let ports = unsafe { &*(raw as *const clap_plugin_note_ports) };
    let count = ports.count.ok_or("plugin note ports have no count")?;
    let get = ports.get.ok_or("plugin note ports have no description")?;
    for index in 0..unsafe { count(plugin, true) } {
        let mut info = MaybeUninit::<clap_note_port_info>::zeroed();
        if !unsafe { get(plugin, index, true, info.as_mut_ptr()) } {
            return Err("plugin rejected a note port description".into());
        }
        let info = unsafe { info.assume_init() };
        if info.supported_dialects & (CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE) == 0 {
            continue;
        }
        let port =
            u16::try_from(index).map_err(|_| "MIDI note port index cannot be represented")?;
        // MIDI 1.0 defines sixteen channels per port. Port count is dynamic.
        for channel in 0..16 {
            result.push(MidiEvent::new(0, port, [0xB0 | channel, 120, 0]));
        }
    }
    Ok(result)
}

fn read_port_list(
    plugin: &clap_sys::plugin::clap_plugin,
    count: unsafe extern "C" fn(
        plugin: *const clap_sys::plugin::clap_plugin,
        is_input: bool,
    ) -> u32,
    get: unsafe extern "C" fn(
        plugin: *const clap_sys::plugin::clap_plugin,
        index: u32,
        is_input: bool,
        info: *mut clap_audio_port_info,
    ) -> bool,
    is_input: bool,
) -> Result<Vec<u32>, String> {
    let port_count = unsafe { count(plugin, is_input) };
    let mut counts = Vec::with_capacity(port_count as usize);
    for index in 0..port_count {
        let mut info = clap_audio_port_info {
            id: 0,
            name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
            flags: 0,
            channel_count: 0,
            port_type: ptr::null(),
            in_place_pair: u32::MAX,
        };
        let ok = unsafe { get(plugin, index, is_input, &mut info) };
        if !ok {
            return Err(format!(
                "plugin did not describe {} port {index}",
                if is_input { "input" } else { "output" }
            ));
        }
        if info.channel_count == 0 {
            return Err(format!(
                "{} port {index} has no channels",
                if is_input { "input" } else { "output" }
            ));
        }
        counts.push(info.channel_count);
    }
    Ok(counts)
}

fn publish_stopped(
    handoff: &ProcessorHandoff,
    stopped: StoppedPluginAudioProcessor<EchodeHost>,
    realtime: bool,
) {
    let Err(stopped) = handoff.try_store(stopped) else {
        return;
    };
    if realtime {
        // The slot already owns a processor. Freeing this one would run on
        // the audio thread. The control thread reclaims the occupant.
        std::mem::forget(stopped);
        return;
    }
    if let Some(previous) = handoff.try_take() {
        drop(previous);
    }
    if let Err(stopped) = handoff.try_store(stopped) {
        drop(stopped);
    }
}

fn stop_loop(bridge: &Bridge) {
    let run_loop = bridge.run_loop.load(Ordering::Acquire);
    if !run_loop.is_null() {
        unsafe { (*run_loop).stop() };
    }
}

fn emit_line(line: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

unsafe extern "C-unwind" fn on_source(info: *mut c_void) {
    if info.is_null() {
        return;
    }
    let bridge = unsafe { &*(info as *const Bridge) };
    bridge.on_main();
}

#[cfg(test)]
pub(crate) fn last_threads() -> Option<(ThreadId, ThreadId, u64)> {
    last_report()
}

#[cfg(test)]
mod tests {
    use super::*;
    use echode_sandbox::MappedRegion;
    use std::time::Duration;

    #[test]
    fn the_run_loop_is_not_the_processing_thread() {
        let path = std::env::temp_dir().join(format!("echode-threads-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _created = MappedRegion::create(&path).expect("region");
        let parent_path = path.clone();
        let parent = thread::spawn(move || {
            let region = MappedRegion::open(&parent_path).expect("open");
            region.wait_ready(Duration::from_secs(2)).expect("ready");
            let input: [&[f32]; 2] = [&[0.25, 0.5], &[0.125, 0.25]];
            let mut left = [0.0f32; 2];
            let mut right = [0.0f32; 2];
            {
                let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
                region
                    .request(1, &input, &mut output, CMD_PROCESS, Duration::from_secs(2))
                    .expect("process");
            }
            assert_eq!(left, [0.25, 0.5]);
            assert_eq!(right, [0.125, 0.25]);
            let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
            region
                .request(2, &input, &mut output, CMD_SHUTDOWN, Duration::from_secs(2))
                .expect("shutdown");
        });
        serve(&path, None, None, 48_000.0, None).expect("serve");
        parent.join().expect("parent");
        let (main, audio, callbacks) = last_threads().expect("report");
        assert_eq!(main, thread::current().id());
        assert_ne!(main, audio);
        assert!(callbacks >= 1, "the main source never ran");
        let _ = std::fs::remove_file(&path);
    }
}
