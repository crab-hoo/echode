//! The fixed wavesynth Sine Pad preset as an ordinary, isolated CLAP instrument.

// The helper runs this on the plugin's audio thread (ADR 0284).
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

use clack_plugin::entry::prelude::*;
use clack_plugin::events::Event as ClapEvent;
use clack_plugin::extensions::prelude::*;
use clack_plugin::prelude::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::note_ports::*;
use clap_sys::plugin::clap_plugin;
use echode_dsp::SinePad as SinePadDsp;
use echode_realtime::Event;
use echode_realtime::plugin_capacity::{PLUGIN_EVENT_CAPACITY, PluginEventCapacity};

#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: echode_dsp::GuardAlloc = echode_dsp::GuardAlloc;

pub struct SinePad;

impl Plugin for SinePad {
    type AudioProcessor<'a> = Processor<'a>;
    type Shared<'a> = Shared;
    type MainThread<'a> = ();

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _: Option<&Shared>) {
        builder.register::<Ports>();
        builder.register::<Notes>();
        builder.register::<Capacity>();
        builder.register::<Tail>();
        builder.register::<State>();
    }
}

impl DefaultPluginFactory for SinePad {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("dev.echode.sine-pad", "Sine Pad")
            .with_vendor("Echode")
            .with_version("0.1.0")
            .with_features([c"instrument", c"synthesizer"])
    }

    fn new_shared(_: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        Ok(Shared::default())
    }

    fn new_main_thread<'a>(_: HostMainThreadHandle<'a>, _: &'a Shared) -> Result<(), PluginError> {
        Ok(())
    }
}

struct Bank {
    synth: SinePadDsp,
    events: Vec<Event>,
    next: *mut Bank,
}

// SAFETY: a bank has one owner; next is only accessed while retired.
unsafe impl Send for Bank {}

impl Bank {
    fn new(rate: u32, capacity: usize) -> Result<Box<Self>, PluginError> {
        let mut synth = SinePadDsp::new(rate).map_err(|_| PluginError::Message("invalid rate"))?;
        synth
            .prepare(capacity, rate)
            .map_err(|_| PluginError::Message("voice preparation failed"))?;
        Ok(Box::new(Self {
            synth,
            events: Vec::with_capacity(capacity),
            next: ptr::null_mut(),
        }))
    }
}

#[derive(Default)]
pub struct Shared {
    rate: AtomicU32,
    capacity: AtomicUsize,
    pending: AtomicPtr<Bank>,
    retired: AtomicPtr<Bank>,
}

impl PluginShared<'_> for Shared {}

impl Shared {
    /// Only the main thread consumes retired banks. Audio only pushes, without
    /// dereferencing an old head, so a concurrently drained list stays safe.
    fn reclaim(&self) {
        let mut bank = self.retired.swap(ptr::null_mut(), Ordering::AcqRel);
        while !bank.is_null() {
            // SAFETY: swapping the list transfers every node to the main thread.
            let old = unsafe { Box::from_raw(bank) };
            bank = old.next;
        }
    }

    fn prepare(&self, capacity: usize) -> Result<(), PluginError> {
        self.reclaim();
        if capacity <= self.capacity.load(Ordering::Acquire) {
            return Ok(());
        }
        let bank = Bank::new(self.rate.load(Ordering::Acquire), capacity)?;
        let old = self.pending.swap(Box::into_raw(bank), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: audio never took this unpublished bank.
            drop(unsafe { Box::from_raw(old) });
        }
        self.capacity.store(capacity, Ordering::Release);
        Ok(())
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.reclaim();
        let pending = *self.pending.get_mut();
        if !pending.is_null() {
            // SAFETY: the processor is gone before Shared is dropped.
            drop(unsafe { Box::from_raw(pending) });
        }
    }
}

pub struct Processor<'a> {
    shared: &'a Shared,
    bank: Box<Bank>,
    rate: u32,
}

impl<'a> PluginAudioProcessor<'a, Shared, ()> for Processor<'a> {
    fn activate(
        _: HostAudioProcessorHandle<'a>,
        _: &(),
        shared: &'a Shared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let rate = config.sample_rate as u32;
        shared.rate.store(rate, Ordering::Release);
        let capacity = shared.capacity.load(Ordering::Acquire).max(1);
        Ok(Self {
            shared,
            bank: Bank::new(rate, capacity)?,
            rate,
        })
    }

    fn reset(&mut self) {
        self.bank.synth.silence();
    }

    fn start_processing(&mut self) -> Result<(), PluginError> {
        // The helper performs its initial start before the realtime loop.
        // Resolve this dylib's debug allocator TLS on that thread before process.
        // Subsequent starts reuse the same thread and initialized TLS.
        let _guard = echode_dsp::ForbidAlloc::enter();
        Ok(())
    }

    fn process(
        &mut self,
        _: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let _guard = echode_dsp::ForbidAlloc::enter();
        let pending = self.shared.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if !pending.is_null() {
            // SAFETY: the atomic swap transfers this prepared bank to audio.
            let mut next = unsafe { Box::from_raw(pending) };
            next.synth.carry_from(&mut self.bank.synth, |_| Some(true));
            let old = Box::into_raw(std::mem::replace(&mut self.bank, next));
            let mut head = self.shared.retired.load(Ordering::Acquire);
            loop {
                // SAFETY: audio exclusively owns old until publishing it.
                unsafe { (*old).next = head };
                match self.shared.retired.compare_exchange_weak(
                    head,
                    old,
                    Ordering::Release,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(current) => head = current,
                }
            }
        }
        self.bank.events.clear();
        for event in events.input.iter() {
            use clack_plugin::events::spaces::CoreEventSpace;
            let converted = match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(note)) => Some(Event::note_on(
                    note.header().time(),
                    0,
                    note.note_id().into_specific().unwrap_or(0),
                    note.key().into_specific().unwrap_or(0) as u8,
                    note.velocity(),
                )),
                Some(CoreEventSpace::NoteOff(note)) => Some(Event::note_off(
                    note.header().time(),
                    0,
                    note.note_id().into_specific().unwrap_or(0),
                    note.key().into_specific().unwrap_or(0) as u8,
                )),
                Some(CoreEventSpace::NoteChoke(_)) => {
                    self.bank.synth.silence();
                    None
                }
                _ => None,
            };
            if let Some(event) = converted {
                if self.bank.events.len() == self.bank.events.capacity() {
                    return Err(PluginError::Message("host event capacity was not prepared"));
                }
                self.bank.events.push(event);
            }
        }
        for mut port in &mut audio {
            let Some(channels) = port.channels()?.into_f32() else {
                continue;
            };
            for channel in channels {
                let output = match channel {
                    ChannelPair::OutputOnly(output)
                    | ChannelPair::InputOutput(_, output)
                    | ChannelPair::InPlace(output) => output,
                    ChannelPair::InputOnly(_) => continue,
                };
                output.fill(0.0);
                self.bank
                    .synth
                    .process(&mut [output], &self.bank.events, self.rate)
                    .map_err(|_| PluginError::Message("Sine Pad process failed"))?;
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

#[derive(Clone, Copy)]
struct Capacity;
// SAFETY: `PLUGIN_EVENT_CAPACITY` is the identifier of Echode's own extension this type stands for.
unsafe impl Extension for Capacity {
    const IDENTIFIERS: &[&CStr] = &[PLUGIN_EVENT_CAPACITY];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `PluginEventCapacity`, the `#[repr(C)]` table declared for
// that identifier.
unsafe impl ExtensionImplementation<SinePad> for Capacity {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&PluginEventCapacity {
            prepare: prepare_capacity,
        });
}
unsafe extern "C" fn prepare_capacity(plugin: *const c_void, events: u32) -> bool {
    // SAFETY: the host calls this extension on the plugin main thread.
    unsafe {
        PluginWrapper::<SinePad>::handle(plugin.cast(), |wrapper| {
            wrapper.shared().prepare(events.max(1) as usize)?;
            Ok(true)
        })
    }
    .unwrap_or(false)
}

#[derive(Clone, Copy)]
struct Ports;
// SAFETY: `CLAP_EXT_AUDIO_PORTS` is the official identifier of the extension this type stands for.
unsafe impl Extension for Ports {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_audio_ports`, the `#[repr(C)]` table CLAP
// declares for that identifier.
unsafe impl ExtensionImplementation<SinePad> for Ports {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_audio_ports {
            count: Some(port_count),
            get: Some(port_get),
        });
}
unsafe extern "C" fn port_count(_: *const clap_plugin, input: bool) -> u32 {
    u32::from(!input)
}
unsafe extern "C" fn port_get(
    _: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if input || index != 0 || info.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies writable storage for this descriptor.
    unsafe {
        *info = clap_audio_port_info {
            id: 0,
            name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
            flags: CLAP_AUDIO_PORT_IS_MAIN,
            channel_count: 1,
            port_type: CLAP_PORT_MONO.as_ptr(),
            in_place_pair: u32::MAX,
        }
    };
    true
}

#[derive(Clone, Copy)]
struct Notes;
// SAFETY: `CLAP_EXT_NOTE_PORTS` is the official identifier of the extension this type stands for.
unsafe impl Extension for Notes {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_NOTE_PORTS];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_note_ports`, the `#[repr(C)]` table CLAP
// declares for that identifier.
unsafe impl ExtensionImplementation<SinePad> for Notes {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_note_ports {
            count: Some(note_count),
            get: Some(note_get),
        });
}
unsafe extern "C" fn note_count(_: *const clap_plugin, input: bool) -> u32 {
    u32::from(input)
}
unsafe extern "C" fn note_get(
    _: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    if !input || index != 0 || info.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies writable storage for this descriptor.
    unsafe {
        *info = clap_note_port_info {
            id: 0,
            supported_dialects: CLAP_NOTE_DIALECT_CLAP,
            preferred_dialect: CLAP_NOTE_DIALECT_CLAP,
            name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
        }
    };
    true
}

clack_export_entry!(SinglePluginEntry<SinePad>);

#[derive(Clone, Copy)]
struct Tail;
// SAFETY: `CLAP_EXT_TAIL` is the official identifier of the extension this type stands for.
unsafe impl Extension for Tail {
    const IDENTIFIERS: &[&CStr] = &[clap_sys::ext::tail::CLAP_EXT_TAIL];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_tail`, the `#[repr(C)]` table CLAP declares for
// that identifier.
unsafe impl ExtensionImplementation<SinePad> for Tail {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_sys::ext::tail::clap_plugin_tail { get: Some(tail) });
}
unsafe extern "C" fn tail(plugin: *const clap_plugin) -> u32 {
    // SAFETY: CLAP permits this query on the main or audio thread.
    unsafe {
        PluginWrapper::<SinePad>::handle(plugin, |wrapper| {
            Ok(wrapper
                .shared()
                .rate
                .load(Ordering::Acquire)
                .saturating_mul(400)
                / 1000)
        })
    }
    .unwrap_or(0)
}

#[derive(Clone, Copy)]
struct State;
// SAFETY: `CLAP_EXT_STATE` is the official identifier of the extension this type stands for.
unsafe impl Extension for State {
    const IDENTIFIERS: &[&CStr] = &[clap_sys::ext::state::CLAP_EXT_STATE];
    type ExtensionSide = PluginExtensionSide;
    unsafe fn from_raw(_: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}
// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_state`, the `#[repr(C)]` table CLAP declares
// for that identifier.
unsafe impl ExtensionImplementation<SinePad> for State {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_sys::ext::state::clap_plugin_state {
            save: Some(save),
            load: Some(load),
        });
}
const PRESET_STATE: &[u8] = b"SinePad1";
unsafe extern "C" fn save(
    _: *const clap_plugin,
    stream: *const clap_sys::stream::clap_ostream,
) -> bool {
    if stream.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies this stream for the duration of the call.
    let Some(write) = (unsafe { (*stream).write }) else {
        return false;
    };
    let mut rest = PRESET_STATE;
    while !rest.is_empty() {
        // SAFETY: `write` is the stream's own function, given the stream and `rest`, readable for
        // its length.
        let wrote = unsafe { write(stream, rest.as_ptr().cast(), rest.len() as u64) };
        // A count past what is left is a broken stream, like a failed write.
        let Some(after) = usize::try_from(wrote)
            .ok()
            .filter(|wrote| *wrote > 0)
            .and_then(|wrote| rest.get(wrote..))
        else {
            return false;
        };
        rest = after;
    }
    true
}
unsafe extern "C" fn load(
    _: *const clap_plugin,
    stream: *const clap_sys::stream::clap_istream,
) -> bool {
    if stream.is_null() {
        return false;
    }
    // This fixed buffer describes the versioned preset state, not a product limit.
    let mut bytes = [0; 8];
    // SAFETY: the host passes a non-null input stream, checked above, valid for the call.
    let Some(read) = (unsafe { (*stream).read }) else {
        return false;
    };
    let mut rest: &mut [u8] = &mut bytes;
    while !rest.is_empty() {
        // SAFETY: `read` is the stream's own function, given the stream and `rest`, writable for
        // its length.
        let count = unsafe { read(stream, rest.as_mut_ptr().cast(), rest.len() as u64) };
        let Some(after) = usize::try_from(count)
            .ok()
            .filter(|count| *count > 0)
            .and_then(|count| std::mem::take(&mut rest).get_mut(count..))
        else {
            return false;
        };
        rest = after;
    }
    bytes == PRESET_STATE
}
