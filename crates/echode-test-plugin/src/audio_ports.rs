//! Probes dimension-preserving rescans and deliberately invalid layout changes.
use clack_plugin::extensions::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_AUDIO_PORT_PREFERS_64BITS,
    CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE, CLAP_AUDIO_PORT_SUPPORTS_64BITS,
    CLAP_AUDIO_PORTS_RESCAN_FLAGS, CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR,
    CLAP_AUDIO_PORTS_RESCAN_NAMES, CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE, CLAP_EXT_AUDIO_PORTS,
    CLAP_PORT_MONO, CLAP_PORT_STEREO, clap_audio_port_info, clap_host_audio_ports,
};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const FAILED: u8 = 0x80;
const SUPPORTED: u32 = CLAP_AUDIO_PORTS_RESCAN_NAMES
    | CLAP_AUDIO_PORTS_RESCAN_FLAGS
    | CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE
    | CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR;
static STAGES: AtomicU8 = AtomicU8::new(0);
static ACTIVATIONS: AtomicU8 = AtomicU8::new(0);
static READS: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LAYOUT: AtomicU8 = AtomicU8::new(0);
static WANTED: AtomicU8 = AtomicU8::new(0);

pub(crate) fn probe(host: HostSharedHandle<'_>, stage: u8) {
    let Some(extension) = host.get_extension::<HostAudioPorts>() else {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
        return;
    };
    // SAFETY: the live host owns this table; initialization, the main callback,
    // and deactivate all run on main. Only NAMES is sent while active.
    unsafe {
        let table = extension.0.as_ptr().as_ref();
        let host = extension.0.host_ptr().as_ptr();
        let (Some(supported), Some(rescan)) = (table.is_rescan_flag_supported, table.rescan) else {
            STAGES.fetch_or(FAILED, Ordering::Relaxed);
            return;
        };
        let passed = (0..u32::BITS).all(|bit| {
            let flag = 1u32 << bit;
            supported(host, flag) == (flag & SUPPORTED != 0)
        }) && !supported(host, 0)
            && !supported(host, u32::MAX)
            && !supported(host, SUPPORTED);
        STAGES.fetch_or(if passed { stage } else { FAILED }, Ordering::Relaxed);
        rescan(
            host,
            if ACTIVE.load(Ordering::Relaxed) {
                CLAP_AUDIO_PORTS_RESCAN_NAMES
            } else {
                SUPPORTED
            },
        );
    }
}

pub(crate) fn activated() {
    let activations = ACTIVATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!("audio-port-activation {activations}");
    // The valid modes each have one input and one output. This small counter
    // only bounds this fixture's restart sequence, not product port counts.
    if READS.load(Ordering::Relaxed) != activations * 2 {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
    }
    ACTIVE.store(true, Ordering::Relaxed);
}

pub(crate) fn deactivate(host: HostSharedHandle<'_>) {
    ACTIVE.store(false, Ordering::Relaxed);
    LAYOUT.store(WANTED.load(Ordering::Relaxed), Ordering::Relaxed);
    probe(host, 4);
}

pub(crate) fn change(host: HostSharedHandle<'_>, layout: u8) {
    WANTED.store(layout, Ordering::Relaxed);
    host.request_restart();
}

pub(crate) fn ports() -> (&'static [u32], &'static [u32]) {
    if ACTIVE.load(Ordering::Relaxed) {
        STAGES.fetch_or(FAILED, Ordering::Relaxed);
    }
    match LAYOUT.load(Ordering::Relaxed) {
        2 => (&[1], &[2]),
        3 => (&[1, 1], &[2]), // Same flattened size, different port boundaries.
        6 => (&[2], &[1]),
        7 => (&[], &[]),
        _ => (&[2], &[2]),
    }
}

pub(crate) fn get(index: u32, is_input: bool, info: &mut clap_audio_port_info) -> bool {
    let (inputs, outputs) = ports();
    let Some(&channels) = (if is_input { inputs } else { outputs }).get(index as usize) else {
        return false;
    };
    READS.fetch_add(1, Ordering::Relaxed);
    let layout = LAYOUT.load(Ordering::Relaxed);
    if layout == 5 && !is_input {
        return false;
    }
    let changed = layout == 1;
    *info = clap_audio_port_info {
        id: index + u32::from((layout == 4 && is_input) || (layout == 8 && !is_input)),
        name: name(),
        flags: if index == 0 {
            CLAP_AUDIO_PORT_IS_MAIN
        } else {
            0
        } | if changed {
            CLAP_AUDIO_PORT_SUPPORTS_64BITS
                | CLAP_AUDIO_PORT_PREFERS_64BITS
                | CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE
        } else {
            0
        },
        channel_count: channels,
        port_type: if changed {
            std::ptr::null()
        } else if channels == 1 {
            CLAP_PORT_MONO.as_ptr()
        } else {
            CLAP_PORT_STEREO.as_ptr()
        },
        in_place_pair: if changed { 0 } else { u32::MAX },
    };
    true
}

pub(crate) fn snapshot() -> [u8; 4] {
    [
        STAGES.load(Ordering::Relaxed),
        ACTIVATIONS.load(Ordering::Relaxed),
        READS.load(Ordering::Relaxed),
        LAYOUT.load(Ordering::Relaxed),
    ]
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

#[derive(Clone, Copy)]
struct HostAudioPorts(RawExtension<HostExtensionSide, clap_host_audio_ports>);
// SAFETY: this identifier selects the official CLAP host audio-ports table.
unsafe impl Extension for HostAudioPorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS];
    type ExtensionSide = HostExtensionSide;
    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: the identifier selects the clap_host_audio_ports C layout.
        Self(unsafe { raw.cast() })
    }
}
