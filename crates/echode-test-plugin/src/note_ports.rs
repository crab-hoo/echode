//! Exercises native dialect discovery and inactive-only note-port changes.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI2,
    CLAP_NOTE_PORTS_RESCAN_ALL, CLAP_NOTE_PORTS_RESCAN_NAMES, clap_host_note_ports,
};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const FAILED: u8 = 0x80;
static STAGES: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTIVATIONS: AtomicU8 = AtomicU8::new(0);
static PORTS: AtomicU8 = AtomicU8::new(1);
static WANTED: AtomicU8 = AtomicU8::new(1);

pub(crate) fn probe(host: HostSharedHandle<'_>, stage: u8) {
    let Some(extension) = host.get_extension::<HostNotePorts>() else {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        return;
    };
    // SAFETY: this is the live host's official table, called on main during
    // initialization, on_main_thread, or deactivate. ALL is inactive-only.
    unsafe {
        let table = extension.0.as_ptr().as_ref();
        let host = extension.0.host_ptr().as_ptr();
        let (Some(dialects), Some(rescan)) = (table.supported_dialects, table.rescan) else {
            STAGES.fetch_or(FAILED, Ordering::Relaxed);
            return;
        };
        let passed = dialects(host)
            == CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI2;
        STAGES.fetch_or(if passed { stage } else { FAILED }, Ordering::Relaxed);
        rescan(host, CLAP_NOTE_PORTS_RESCAN_NAMES);
        if !ACTIVE.load(Ordering::Relaxed) {
            rescan(
                host,
                CLAP_NOTE_PORTS_RESCAN_ALL | CLAP_NOTE_PORTS_RESCAN_NAMES,
            );
        }
    }
}

pub(crate) fn activate() {
    ACTIVE.store(true, Ordering::Relaxed);
    ACTIVATIONS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn deactivate(host: HostSharedHandle<'_>) {
    ACTIVE.store(false, Ordering::Relaxed);
    PORTS.store(WANTED.load(Ordering::Relaxed), Ordering::Relaxed);
    probe(host, 4);
}

pub(crate) fn change(host: HostSharedHandle<'_>, ports: u8) {
    WANTED.store(ports, Ordering::Relaxed);
    host.request_restart();
}

pub(crate) fn count(is_input: bool) -> u32 {
    if ACTIVE.load(Ordering::Relaxed) {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
    }
    if is_input {
        u32::from(PORTS.load(Ordering::Relaxed))
    } else {
        0
    }
}

pub(crate) fn name() -> [std::ffi::c_char; clap_sys::string_sizes::CLAP_NAME_SIZE] {
    let mut name = [0; clap_sys::string_sizes::CLAP_NAME_SIZE];
    name[0] = if STAGES.load(Ordering::Relaxed) & 2 == 0 {
        b'A'
    } else {
        b'B'
    } as std::ffi::c_char;
    name
}

pub(crate) fn port_dialects(index: u32) -> u32 {
    // These ports also take CLAP, so MIDI carries only lifecycle resets here.
    CLAP_NOTE_DIALECT_CLAP
        | if index.is_multiple_of(2) {
            CLAP_NOTE_DIALECT_MIDI
        } else {
            0
        }
}

pub(crate) fn snapshot() -> [u8; 3] {
    [
        STAGES.load(Ordering::Relaxed),
        ACTIVATIONS.load(Ordering::Relaxed),
        PORTS.load(Ordering::Relaxed),
    ]
}

pub(crate) fn render(events: &clack_plugin::events::io::InputEvents<'_>) -> f32 {
    use clack_plugin::events::spaces::CoreEventSpace;
    let mut resets = 0u64;
    let mut note = false;
    let mut choke = false;
    for event in events {
        match event.as_core_event() {
            Some(CoreEventSpace::NoteChoke(_)) if !note && resets == 0 => choke = true,
            Some(CoreEventSpace::Midi(midi)) if !note && choke => {
                let port = u32::from(midi.port_index());
                let data = midi.data();
                if port >= u32::from(PORTS.load(Ordering::Relaxed))
                    || !port.is_multiple_of(2)
                    || data[0] & 0xF0 != 0xB0
                    || data[1..] != [120, 0]
                {
                    return -1.0;
                }
                // This fixture covers at most three ports and sixteen MIDI
                // channels, not a product bound. Detect duplicates and omissions.
                let bit = 1u64 << (port * 16 + u32::from(data[0] & 0x0F));
                if resets & bit != 0 {
                    return -2.0;
                }
                resets |= bit;
            }
            Some(CoreEventSpace::NoteOn(on)) if on.key().into_specific() == Some(60) => note = true,
            _ => return -3.0,
        }
    }
    let ports = PORTS.load(Ordering::Relaxed);
    let expected = (0..ports)
        .filter(|port| port.is_multiple_of(2))
        .fold(0u64, |bits, port| {
            bits | (0xFFFFu64 << (u32::from(port) * 16))
        });
    if (note || ports == 0) && (!choke || resets == expected) {
        0.25
    } else {
        -4.0
    }
}

#[derive(Clone, Copy)]
struct HostNotePorts(RawExtension<HostExtensionSide, clap_host_note_ports>);
// SAFETY: this identifier selects the official host note-ports table.
unsafe impl Extension for HostNotePorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_NOTE_PORTS];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the clap_host_note_ports C layout.
        Self(unsafe { raw.cast() })
    }
}
