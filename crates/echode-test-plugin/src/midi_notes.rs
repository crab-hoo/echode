//! Records how project notes reach MIDI-only and CLAP-only note input ports.
use clack_plugin::events::io::InputEvents;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI2,
    clap_host_note_ports,
};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Two input ports show native and MIDI dialects at once.
pub(crate) const PORTS: u32 = 2;
/// Bit n selects MIDI for port n; bit 7 selects MIDI 2.0 instead of MIDI 1.0.
static LAYOUT: AtomicU8 = AtomicU8::new(0b10);
/// The layout a state load asked for, applied while inactive.
static WANTED: AtomicU8 = AtomicU8::new(0b10);
static ACTIVATIONS: AtomicU8 = AtomicU8::new(0);
static HOST_DIALECTS: AtomicU32 = AtomicU32::new(u32::MAX);
/// Sixty-four hold this fixture's reset and note blocks, not a product limit.
static RECORDS: [Record; 64] = [const { Record::new() }; 64];
/// Latest nonempty block's record count, or u8::MAX for an overflow.
static COUNT: AtomicU8 = AtomicU8::new(0);

struct Record {
    address: AtomicU64,
    words: [AtomicU32; 4],
}

impl Record {
    const fn new() -> Self {
        Self {
            address: AtomicU64::new(0),
            words: [const { AtomicU32::new(0) }; 4],
        }
    }
}

const NATIVE_ON: u64 = 1;
const NATIVE_OFF: u64 = 2;
const NATIVE_CHOKE: u64 = 3;
const MIDI: u64 = 4;
const EXPRESSION: u64 = 5;
const OTHER: u64 = 6;
const MIDI2: u64 = 7;

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
    let layout = LAYOUT.load(Ordering::Acquire);
    if u32::from(layout) & (1 << index) == 0 {
        CLAP_NOTE_DIALECT_CLAP
    } else if layout & 0x80 != 0 {
        CLAP_NOTE_DIALECT_MIDI2
    } else {
        CLAP_NOTE_DIALECT_MIDI
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
    LAYOUT.store(WANTED.load(Ordering::Acquire), Ordering::Release);
}

pub(crate) fn record(events: &InputEvents<'_>) {
    let total = events.len() as usize;
    if total == 0 {
        return;
    }
    for (slot, event) in RECORDS.iter().zip(events) {
        let (kind, port, data) = match event.as_core_event() {
            Some(CoreEventSpace::Midi(midi)) => {
                let [status, key, value] = midi.data();
                (
                    MIDI,
                    midi.port_index(),
                    [
                        u32::from(status) | u32::from(key) << 8 | u32::from(value) << 16,
                        0,
                        0,
                        0,
                    ],
                )
            }
            Some(CoreEventSpace::Midi2(midi)) => (MIDI2, midi.port_index(), midi.data()),
            Some(CoreEventSpace::NoteOn(note)) => native(NATIVE_ON, note.pckn()),
            Some(CoreEventSpace::NoteOff(note)) => native(NATIVE_OFF, note.pckn()),
            Some(CoreEventSpace::NoteChoke(note)) => native(NATIVE_CHOKE, note.pckn()),
            Some(CoreEventSpace::NoteExpression(note)) => native(EXPRESSION, note.pckn()),
            _ => (OTHER, u16::MAX, [0; 4]),
        };
        slot.address.store(
            kind | u64::from(port) << 8 | u64::from(event.header().time()) << 24,
            Ordering::Release,
        );
        for (word, value) in slot.words.iter().zip(data) {
            word.store(value, Ordering::Release);
        }
    }
    COUNT.store(
        if total > RECORDS.len() {
            u8::MAX
        } else {
            total as u8
        },
        Ordering::Release,
    );
}

fn native(kind: u64, pckn: clack_plugin::events::Pckn) -> (u64, u16, [u32; 4]) {
    let port = pckn.port_index.into_specific().unwrap_or(u16::MAX);
    let key = pckn.key.into_specific().unwrap_or(0xFF);
    (kind, port, [key as u32, 0, 0, 0])
}

/// Dialects, activations, layout, count; each record is an address and four words.
pub(crate) fn snapshot() -> Vec<u8> {
    let mut bytes = vec![
        HOST_DIALECTS.load(Ordering::Acquire) as u8,
        ACTIVATIONS.load(Ordering::Acquire),
        LAYOUT.load(Ordering::Acquire),
        COUNT.load(Ordering::Acquire),
    ];
    for record in &RECORDS {
        bytes.extend_from_slice(&record.address.load(Ordering::Acquire).to_le_bytes());
        for word in &record.words {
            bytes.extend_from_slice(&word.load(Ordering::Acquire).to_le_bytes());
        }
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
