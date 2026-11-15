//! Records how project notes reach MIDI-only and CLAP-only note input ports.
use clack_plugin::events::io::InputEvents;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, clap_host_note_ports,
};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Input note ports this fixture declares. Two show both dialects at once.
pub(crate) const PORTS: u32 = 2;
/// Bit `n` set: input port `n` takes MIDI only. Clear: CLAP only.
static MIDI_ONLY: AtomicU8 = AtomicU8::new(0b10);
/// The layout a state load asked for, applied while inactive.
static WANTED: AtomicU8 = AtomicU8::new(0b10);
static ACTIVATIONS: AtomicU8 = AtomicU8::new(0);
/// The host's `supported_dialects` at initialization.
static HOST_DIALECTS: AtomicU32 = AtomicU32::new(u32::MAX);
/// The events of the latest block that had any, one packed record each.
/// Sixteen cover this fixture's blocks; they bound no product event count.
static RECORDS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Records in `RECORDS`, or `u8::MAX` when a block had more than it holds.
static COUNT: AtomicU8 = AtomicU8::new(0);

const NATIVE_ON: u64 = 1;
const NATIVE_OFF: u64 = 2;
const NATIVE_CHOKE: u64 = 3;
const MIDI: u64 = 4;
const EXPRESSION: u64 = 5;
const OTHER: u64 = 6;

pub(crate) fn probe(host: HostSharedHandle<'_>) {
    let Some(extension) = host.get_extension::<HostNotePorts>() else {
        return;
    };
    // SAFETY: the live host's official table, called on main during initialization.
    let dialects = unsafe {
        let table = extension.0.as_ptr().as_ref();
        table
            .supported_dialects
            .map(|dialects| dialects(extension.0.host_ptr().as_ptr()))
    };
    if let Some(dialects) = dialects {
        HOST_DIALECTS.store(dialects, Ordering::Release);
    }
}

pub(crate) fn dialects(index: u32) -> u32 {
    if u32::from(MIDI_ONLY.load(Ordering::Acquire)) & (1 << index) != 0 {
        CLAP_NOTE_DIALECT_MIDI
    } else {
        CLAP_NOTE_DIALECT_CLAP
    }
}

pub(crate) fn change(host: HostSharedHandle<'_>, layout: u8) {
    WANTED.store(layout, Ordering::Release);
    host.request_restart();
}

pub(crate) fn activate() {
    ACTIVATIONS.fetch_add(1, Ordering::AcqRel);
}

pub(crate) fn deactivate() {
    // CLAP allows a note-port change only while inactive.
    MIDI_ONLY.store(WANTED.load(Ordering::Acquire), Ordering::Release);
}

pub(crate) fn record(events: &InputEvents<'_>) {
    let total = events.len() as usize;
    if total == 0 {
        return;
    }
    for (slot, event) in RECORDS.iter().zip(events) {
        let time = u64::from(event.header().time() as u16);
        let (kind, port, data) = match event.as_core_event() {
            Some(CoreEventSpace::Midi(midi)) => {
                let [status, key, value] = midi.data();
                let data = u64::from(status) | u64::from(key) << 8 | u64::from(value) << 16;
                (MIDI, midi.port_index(), data)
            }
            Some(CoreEventSpace::NoteOn(note)) => native(NATIVE_ON, note.pckn()),
            Some(CoreEventSpace::NoteOff(note)) => native(NATIVE_OFF, note.pckn()),
            Some(CoreEventSpace::NoteChoke(note)) => native(NATIVE_CHOKE, note.pckn()),
            Some(CoreEventSpace::NoteExpression(note)) => native(EXPRESSION, note.pckn()),
            _ => (OTHER, u16::MAX, 0),
        };
        let port = u64::from(port as u8);
        slot.store(
            kind | port << 8 | time << 16 | data << 32,
            Ordering::Release,
        );
    }
    let count = if total > RECORDS.len() {
        u8::MAX
    } else {
        total as u8
    };
    COUNT.store(count, Ordering::Release);
}

fn native(kind: u64, pckn: clack_plugin::events::Pckn) -> (u64, u16, u64) {
    let port = pckn.port_index.into_specific().unwrap_or(u16::MAX);
    let key = pckn.key.into_specific().unwrap_or(0xFF);
    (kind, port, u64::from(key as u8))
}

/// Host dialects, activations, current layout, record count, then each record.
pub(crate) fn snapshot() -> Vec<u8> {
    let mut bytes = vec![
        HOST_DIALECTS.load(Ordering::Acquire) as u8,
        ACTIVATIONS.load(Ordering::Acquire),
        MIDI_ONLY.load(Ordering::Acquire),
        COUNT.load(Ordering::Acquire),
    ];
    for record in &RECORDS {
        bytes.extend_from_slice(&record.load(Ordering::Acquire).to_le_bytes());
    }
    bytes
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
