//! Main run loop, one realtime processing thread, and one control thread.
//!
//! The processing thread never waits for `deactivate` / `activate`. Those run
//! on the main thread. A shared alarm wakes the processing thread both for a
//! parent block and for a restart that arrives while the transport is idle.

use crate::events::write_input;
use crate::{HostError, load_entry};
use clack_host::events::io::{EventBuffer, InputEvents};
use clack_host::prelude::*;
use echode_realtime::Event;
use echode_sandbox::{
    CMD_PROCESS, CMD_SHUTDOWN, ControlMessage, FanIn, MappedRegion, RegionError, Socket, SyncFlags,
    wake_one,
};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopCommonModes,
    kCFRunLoopDefaultMode,
};
use std::cell::Cell;
use std::ffi::c_void;
use std::os::fd::OwnedFd;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::thread::{self, ThreadId};

const OP_RESTART: u8 = 1;
const OP_ARM: u8 = 2;

thread_local! {
    static INSTANCE: Cell<*mut PluginInstance<EchodeHost>> = const { Cell::new(ptr::null_mut()) };
}

struct Bridge {
    source: AtomicPtr<CFRunLoopSource>,
    run_loop: AtomicPtr<CFRunLoop>,
    current: AtomicPtr<MappedRegion>,
    pending: AtomicPtr<MappedRegion>,
    retired: AtomicPtr<MappedRegion>,
    switched: AtomicU64,
    map_lock: AtomicBool,
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
    slot: AtomicPtr<StoppedPluginAudioProcessor<EchodeHost>>,
    callback_gen: AtomicU64,
    callback_seen: AtomicU64,
    process_note: AtomicBool,
    stages: AtomicU32,
    fan: AtomicPtr<FanIn>,
    sample_bits: AtomicU64,
    activation_frames: AtomicU32,
    // Read after `serve` returns, from the unit test.
    #[allow(dead_code)]
    main_id: std::sync::Mutex<Option<ThreadId>>,
    #[allow(dead_code)]
    audio_id: std::sync::Mutex<Option<ThreadId>>,
    #[allow(dead_code)]
    callbacks: AtomicU64,
}

// SAFETY: the CoreFoundation pointers stay valid until both worker threads have
// been joined. Mapping pointers are swapped under `map_lock`, and the previous
// mapping is freed only after the processing thread has left it.
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

    fn lock_map(&self) {
        while self
            .map_lock
            .compare_exchange_weak(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            std::hint::spin_loop();
        }
    }

    fn unlock_map(&self) {
        self.map_lock.store(false, Ordering::Release);
    }

    fn bump_audio(&self) {
        self.lock_map();
        let region = self.current.load(Ordering::Acquire);
        if !region.is_null() {
            // SAFETY: `current` is not freed while `map_lock` is held.
            unsafe { (*region).bump_alarm() };
        }
        self.unlock_map();
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
            if let Some(stopped) = take_stopped(&self.slot) {
                instance.deactivate(stopped);
                match activate(
                    instance,
                    f64::from_bits(self.sample_bits.load(Ordering::Acquire)),
                    self.activation_frames.load(Ordering::Acquire),
                ) {
                    Ok(stopped) => publish_stopped(&self.slot, stopped),
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
            if self.slot.load(Ordering::Acquire).is_null() {
                match activate(
                    instance,
                    f64::from_bits(self.sample_bits.load(Ordering::Acquire)),
                    self.activation_frames.load(Ordering::Acquire),
                ) {
                    Ok(stopped) => publish_stopped(&self.slot, stopped),
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

impl HostHandlers for EchodeHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
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
    control: Option<Socket>,
    fan: Option<FanIn>,
    plugin: Option<&std::path::Path>,
    plugin_id: Option<&str>,
    sample_rate: f64,
    workgroup: Option<(&str, u32)>,
) -> Result<(), HostError> {
    let max_frames = region.layout().max_frames;
    let region = Box::into_raw(Box::new(region));
    let fan = fan.map(|fan| Box::into_raw(Box::new(fan)));
    let bridge = Arc::new(Bridge {
        source: AtomicPtr::new(ptr::null_mut()),
        run_loop: AtomicPtr::new(ptr::null_mut()),
        current: AtomicPtr::new(region),
        pending: AtomicPtr::new(ptr::null_mut()),
        retired: AtomicPtr::new(ptr::null_mut()),
        switched: AtomicU64::new(0),
        map_lock: AtomicBool::new(false),
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
        slot: AtomicPtr::new(ptr::null_mut()),
        callback_gen: AtomicU64::new(0),
        callback_seen: AtomicU64::new(0),
        process_note: AtomicBool::new(false),
        stages: AtomicU32::new(1),
        fan: AtomicPtr::new(fan.unwrap_or(ptr::null_mut())),
        sample_bits: AtomicU64::new(sample_rate.to_bits()),
        activation_frames: AtomicU32::new(max_frames.max(1)),
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
        Some(path) => Some(load_plugin(path, plugin_id, Arc::clone(&bridge))?),
    };
    if let Some(loaded) = loaded.as_mut() {
        let stopped = activate(&mut loaded.instance, sample_rate, max_frames)?;
        publish_stopped(&bridge.slot, stopped);
        bridge.need_start.store(true, Ordering::Release);
        INSTANCE.with(|cell| cell.set(ptr::from_mut(&mut loaded.instance)));
    }

    let workgroup = workgroup.map(|(name, ticks)| (name.to_string(), ticks));
    let bridge_audio = Arc::clone(&bridge);
    let bridge_control = Arc::clone(&bridge);
    let audio_result = thread::scope(|scope| {
        let audio = scope.spawn(|| {
            if let Ok(mut slot) = bridge_audio.audio_id.lock() {
                *slot = Some(thread::current().id());
            }
            audio_thread(&bridge_audio, workgroup.as_ref())
        });
        let control_handle = scope.spawn(|| control_thread(&bridge_control, control));
        bridge.signal();
        let mode = unsafe { kCFRunLoopDefaultMode };
        while !bridge.stop.load(Ordering::Acquire) {
            let _ = CFRunLoop::run_in_mode(mode, 0.05, true);
        }
        audio
            .join()
            .expect("processing thread")
            .and(control_handle.join().expect("control thread"))
    });

    INSTANCE.with(|cell| cell.set(ptr::null_mut()));
    if let Some(loaded) = loaded.as_mut()
        && let Some(stopped) = take_stopped(&bridge.slot)
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
    let fan = bridge.fan.swap(ptr::null_mut(), Ordering::AcqRel);
    if !fan.is_null() {
        drop(unsafe { Box::from_raw(fan) });
    }
    audio_result
}

fn release_maps(bridge: &Bridge) {
    bridge.lock_map();
    let maps = [
        bridge.current.swap(ptr::null_mut(), Ordering::AcqRel),
        bridge.pending.swap(ptr::null_mut(), Ordering::AcqRel),
        bridge.retired.swap(ptr::null_mut(), Ordering::AcqRel),
    ];
    bridge.unlock_map();
    for pointer in maps {
        if !pointer.is_null() {
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
        |_| (),
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
        return Err(error);
    }
    bridge.lock_map();
    let initial = bridge.current.load(Ordering::Acquire);
    if !initial.is_null() {
        // SAFETY: the initial mapping is installed before this thread starts and
        // is not replaced until `adopt` runs, which has not yet.
        unsafe { (*initial).mark_ready() };
    }
    let mut seen_alarm = if initial.is_null() {
        0
    } else {
        unsafe { (*initial).alarm() }
    };
    bridge.unlock_map();
    let mut seen_request = 0u64;
    // Allocated once. `process` reuses them and does not allocate per block.
    let (channels, frames, event_cap) = {
        let region = current_region(bridge);
        (
            region.input_channels().max(region.output_channels()).max(1),
            region.layout().max_frames as usize,
            region.layout().input_events as usize,
        )
    };
    let mut buffers = PluginBuffers::allocate(channels, frames.max(1), event_cap.max(1));
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
        publish_stopped(&bridge.slot, stopped);
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
    events: EventBuffer,
    decoded: Vec<Event>,
}

impl PluginBuffers {
    /// Sized once from the session. Later blocks reuse this storage.
    fn allocate(channels: usize, frames: usize, events: usize) -> Self {
        Self {
            input_ports: AudioPorts::with_capacity(channels, 1),
            output_ports: AudioPorts::with_capacity(channels, 1),
            input: vec![vec![0.0; frames]; channels],
            output: vec![vec![0.0; frames]; channels],
            events: EventBuffer::new(),
            decoded: Vec::with_capacity(events),
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

fn audio_loop(
    bridge: &Bridge,
    started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>,
    buffers: &mut PluginBuffers,
    seen_request: &mut u64,
    seen_alarm: &mut u64,
) -> Result<(), HostError> {
    loop {
        if adopt(bridge) {
            *seen_request = 0;
            *seen_alarm = current_region(bridge).alarm();
        }
        if bridge.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        pump(bridge, started)?;
        let region = current_region(bridge);
        let published = region.published();
        if published > *seen_request {
            let generation = *seen_request + 1;
            let command = region.block_command(generation);
            if command == CMD_SHUTDOWN {
                region.finish(generation);
                bridge.bump_fan();
                return Ok(());
            }
            if command != CMD_PROCESS {
                return Err(HostError::Region(RegionError::UnknownCommand(command)));
            }
            let frames = region.prepare_block(generation)?;
            let inputs = region.input_channels();
            let outputs = region.output_channels();
            let stages = bridge.stages.load(Ordering::Acquire).max(1);
            let fits = buffers.fits(inputs, outputs, frames);
            if bridge.has_plugin.load(Ordering::Acquire) && fits {
                if let Some(active) = started.as_mut() {
                    for stage in 0..stages {
                        if stage > 0 {
                            region.feed_output_to_input(generation)?;
                        }
                        render_plugin(region, generation, active, frames, buffers)?;
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
            continue;
        }
        let alarm = region.alarm();
        if alarm != *seen_alarm {
            *seen_alarm = alarm;
            continue;
        }
        *seen_alarm = region.wait_alarm(*seen_alarm)?;
    }
}

fn current_region(bridge: &Bridge) -> &MappedRegion {
    let pointer = bridge.current.load(Ordering::Acquire);
    // SAFETY: `current` stays allocated until this thread adopts a replacement
    // or `serve_region` joins this thread and then frees it.
    unsafe { &*pointer }
}

fn adopt(bridge: &Bridge) -> bool {
    bridge.lock_map();
    let pending = bridge.pending.swap(ptr::null_mut(), Ordering::AcqRel);
    if pending.is_null() {
        bridge.unlock_map();
        return false;
    }
    let old = bridge.current.swap(pending, Ordering::AcqRel);
    let leftover = bridge.retired.swap(old, Ordering::AcqRel);
    bridge.switched.fetch_add(1, Ordering::Release);
    bridge.unlock_map();
    bridge.bump_control();
    if !leftover.is_null() {
        drop(unsafe { Box::from_raw(leftover) });
    }
    true
}

fn pump(
    bridge: &Bridge,
    started: &mut Option<StartedPluginAudioProcessor<EchodeHost>>,
) -> Result<(), HostError> {
    let want = bridge.restart_request.load(Ordering::Acquire);
    let acked = bridge.restart_ack.load(Ordering::Acquire);
    if want != acked {
        if let Some(active) = started.take() {
            let stopped = active.stop_processing();
            publish_stopped(&bridge.slot, stopped);
            bridge.holds_started.store(false, Ordering::Release);
        }
        bridge.restart_ack.store(want, Ordering::Release);
        bridge.bump_control();
    } else if bridge.need_start.swap(false, Ordering::AcqRel)
        && let Some(stopped) = take_stopped(&bridge.slot)
    {
        match stopped.start_processing() {
            Ok(active) => {
                *started = Some(active);
                bridge.holds_started.store(true, Ordering::Release);
            }
            Err(error) => {
                publish_stopped(&bridge.slot, error.into_stopped_processor());
                bridge.stop.store(true, Ordering::Release);
                return Err(HostError::Plugin(
                    "plugin refused to start processing".to_string(),
                ));
            }
        }
    }
    if bridge.want_process.swap(false, Ordering::AcqRel) && started.is_none() {
        if bridge.slot.load(Ordering::Acquire).is_null() {
            bridge.op.store(OP_ARM, Ordering::Release);
            bridge.signal();
        } else {
            bridge.need_start.store(true, Ordering::Release);
        }
    }
    Ok(())
}

fn control_thread(bridge: &Bridge, mut socket: Option<Socket>) -> Result<(), HostError> {
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
    let mut saved = Vec::new();
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
        if bridge.process_note.swap(false, Ordering::AcqRel)
            && let Some(socket) = &mut socket
        {
            let _ = socket.send(&ControlMessage::RequestProcess);
        }
        if let Some(socket) = &mut socket {
            match socket.try_recv() {
                Ok(Some(message)) => {
                    if handle_control(bridge, socket, &mut saved, message).is_err() {
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
    saved: &mut Vec<u8>,
    message: ControlMessage,
) -> Result<(), HostError> {
    match message {
        ControlMessage::SaveRequest => {
            socket
                .send(&ControlMessage::State(saved.clone()))
                .map_err(|error| HostError::Plugin(error.to_string()))?;
        }
        ControlMessage::Restore(bytes) => {
            *saved = bytes;
            socket
                .send(&ControlMessage::Restored)
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
        ControlMessage::Remap { layout, fd } => match MappedRegion::from_fd(fd) {
            Ok(region) if region.layout() == layout => {
                let pointer = Box::into_raw(Box::new(region));
                let target = bridge.switched.load(Ordering::Acquire) + 1;
                bridge.pending.store(pointer, Ordering::Release);
                bridge.bump_audio();
                while bridge.switched.load(Ordering::Acquire) < target {
                    if bridge.stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    thread::sleep(std::time::Duration::from_millis(1));
                }
                bridge.lock_map();
                let old = bridge.retired.swap(ptr::null_mut(), Ordering::AcqRel);
                bridge.unlock_map();
                if !old.is_null() {
                    drop(unsafe { Box::from_raw(old) });
                }
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
) -> Result<(), HostError> {
    for (channel, plane) in buffers.input.iter_mut().enumerate() {
        unsafe {
            ptr::copy_nonoverlapping(
                region.input_ptr(generation, channel),
                plane.as_mut_ptr(),
                frames,
            );
        }
    }
    let input_audio = buffers.input_ports.with_input_buffers([AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_input_only(
            buffers
                .input
                .iter_mut()
                .map(|channel| InputChannel::variable(&mut channel[..frames])),
        ),
    }]);
    let mut output_audio = buffers.output_ports.with_output_buffers([AudioPortBuffer {
        latency: 0,
        channels: AudioPortBufferType::f32_output_only(
            buffers
                .output
                .iter_mut()
                .map(|channel| &mut channel[..frames]),
        ),
    }]);
    buffers.events.clear();
    region.copy_input_events(generation, &mut buffers.decoded);
    write_input(&buffers.decoded, &mut buffers.events);
    let input_events = InputEvents::from_buffer(&buffers.events);
    let mut output_events = OutputEvents::void();
    started
        .process(
            &input_audio,
            &mut output_audio,
            &input_events,
            &mut output_events,
            None,
            None,
        )
        .map_err(|error| HostError::Plugin(error.to_string()))?;
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

fn publish_stopped(
    slot: &AtomicPtr<StoppedPluginAudioProcessor<EchodeHost>>,
    stopped: StoppedPluginAudioProcessor<EchodeHost>,
) {
    let previous = slot.swap(Box::into_raw(Box::new(stopped)), Ordering::AcqRel);
    if !previous.is_null() {
        drop(unsafe { Box::from_raw(previous) });
    }
}

fn take_stopped(
    slot: &AtomicPtr<StoppedPluginAudioProcessor<EchodeHost>>,
) -> Option<StoppedPluginAudioProcessor<EchodeHost>> {
    let pointer = slot.swap(ptr::null_mut(), Ordering::AcqRel);
    if pointer.is_null() {
        None
    } else {
        Some(*unsafe { Box::from_raw(pointer) })
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
