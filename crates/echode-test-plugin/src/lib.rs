//! Gain of 2.0, plus fault injection for the hosting spike.
//!
//! `ECHODE_TEST_FAULT=crash` aborts inside `process`. `ECHODE_TEST_FAULT=segfault`
//! stores through a null pointer there. `ECHODE_TEST_FAULT=hang`
//! never returns from `process`. Both take down only this plugin's process.
//! `ECHODE_TEST_FAULT=process-wait:<socket>` holds the second process call until
//! a test releases it. The connection is created during activation, off RT.
//! `ECHODE_TEST_FAULT=count-wait:<socket>` holds that call the same way and
//! counts note events as `count` does.
//! `ECHODE_TEST_FAULT=nan` writes 0.5, then NaN from frame 10 of its second
//! block on, until a CLAP reset; afterwards it writes 0.5 again.
//! `ECHODE_TEST_FAULT=huge` writes a finite 2.0e38 to every output.
//! `ECHODE_TEST_FAULT=large-state` saves the state byte repeated
//! [`LARGE_STATE`] times and loads only that shape, marking itself dirty.
//! `ECHODE_TEST_FAULT=delay:<frames>` is a real delay that reports its length
//! as latency; see [`delay`]. `ECHODE_TEST_FAULT=claim:<frames>` reports
//! `<frames>` of latency and delays nothing, as a plugin with a wrong report.
//! `ECHODE_TEST_FAULT=restart-nested` requests a restart in its first block
//! and once more from the activation that restart runs, and prints
//! `nested-activation <n>` at each activation.

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU8, Ordering};

use clack_plugin::entry::prelude::*;
use clack_plugin::extensions::prelude::*;
use clack_plugin::prelude::*;
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_MONO, CLAP_PORT_STEREO,
    clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, clap_note_port_info,
    clap_plugin_note_ports,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state, clap_plugin_state};
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};

mod delay;
mod gui;
mod latency;
mod params;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Gain,
    Instrument,
    Mono,
    Ports,
    NoPorts,
    Flip,
    /// One saved byte: 64 plays 0.25, 128 plays 0.5.
    State,
    /// Refuses a block whose input events go backwards in time.
    Order,
    /// Gain and mode parameters, saved in its state. See [`params`].
    Params,
    /// The gain effect with one mono input and output, for the refusal of a
    /// stereo source folded into a mono input (ADR 0095).
    MonoGain,
}

/// `declare_extensions` can run on another thread than `create_plugin`.
/// The helper hosts one plugin, so the role is process-wide.
static ROLE_BITS: AtomicU8 = AtomicU8::new(0);
/// Fault selected in `create_plugin`, before audio starts. 0 is no fault.
static FAULT_BITS: AtomicU8 = AtomicU8::new(0);

fn role_bits(role: Role) -> u8 {
    match role {
        Role::Gain => 0,
        Role::Instrument => 1,
        Role::Mono => 2,
        Role::Ports => 3,
        Role::NoPorts => 4,
        Role::Flip => 5,
        Role::State => 6,
        Role::Order => 7,
        Role::Params => 8,
        Role::MonoGain => 9,
    }
}

fn role_from_bits(bits: u8) -> Role {
    match bits {
        1 => Role::Instrument,
        2 => Role::Mono,
        3 => Role::Ports,
        4 => Role::NoPorts,
        5 => Role::Flip,
        6 => Role::State,
        7 => Role::Order,
        8 => Role::Params,
        9 => Role::MonoGain,
        _ => Role::Gain,
    }
}

fn set_role(role: Role) {
    ROLE_BITS.store(role_bits(role), Ordering::SeqCst);
}

fn current_role() -> Role {
    role_from_bits(ROLE_BITS.load(Ordering::SeqCst))
}

/// The helper process hosts one plugin, so the saved byte is process-wide.
/// The audio thread reads it and the main thread writes it.
static STATE_BYTE: AtomicU8 = AtomicU8::new(64);

fn port_flip_active() -> bool {
    std::env::temp_dir()
        .join("echode-pro02-port-flip")
        .is_file()
}

pub struct TestGain;

impl Plugin for TestGain {
    type AudioProcessor<'a> = TestGainProcessor<'a>;
    type Shared<'a> = ();
    type MainThread<'a> = TestMain;

    fn declare_extensions(
        builder: &mut PluginExtensions<Self>,
        _shared: Option<&Self::Shared<'_>>,
    ) {
        if current_role() != Role::NoPorts {
            builder.register::<PluginAudioPorts>();
        }
        if matches!(current_role(), Role::State | Role::Params) {
            builder.register::<PluginStateExt>();
        }
        if current_role() == Role::Params {
            builder.register::<params::PluginParamsExt>();
            builder.register::<gui::PluginGuiExt>();
        }
        if current_role() == Role::Order || fault_now() == Fault::TailLegacy {
            builder.register::<PluginNotePorts>();
        }
        if matches!(fault_now(), Fault::Latency | Fault::Delay | Fault::Claim) {
            builder.register::<latency::PluginLatency>();
        }
    }
}

impl DefaultPluginFactory for TestGain {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("dev.echode.test-gain", "Echode Test Gain")
            .with_vendor("Echode")
            .with_version("0.0.0")
            .with_url("https://echode.invalid")
            .with_features([c"audio-effect"])
    }

    fn new_shared(host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        if fault_now() == Fault::Generate {
            return Err(PluginError::Message("generate refused"));
        }
        gui::remember_host(host);
        Ok(())
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        _shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        Ok(TestMain {
            role: current_role(),
            // SAFETY: load runs on this same main thread, while the host is still alive.
            host: unsafe { host.with_arbitrary_lifetime() },
        })
    }
}

pub struct TestMain {
    role: Role,
    host: HostMainThreadHandle<'static>,
}

impl TestMain {
    fn ports(&self) -> (&'static [u32], &'static [u32]) {
        if self.role == Role::Flip && port_flip_active() {
            return (&[], &[1]);
        }
        match self.role {
            Role::Gain | Role::Flip => (&[2], &[2]),
            Role::Instrument => (&[], &[2]),
            Role::Mono => (&[], &[1]),
            Role::MonoGain => (&[1], &[1]),
            Role::Ports => (&[2], &[2, 1]),
            Role::NoPorts => (&[], &[]),
            Role::State | Role::Params => (&[2], &[2]),
            Role::Order => (&[], &[2]),
        }
    }

    fn mark_dirty(&self) {
        let Some(extension) = self.host.get_extension::<SeenHostState>() else {
            return;
        };
        extension.mark_dirty();
    }
}

impl PluginMainThread<'_, ()> for TestMain {
    fn on_main_thread(&self) {
        if gui::CLOSE.swap(false, Ordering::AcqRel) {
            gui::report_closed();
            return;
        }
        if gui::DIRTY.swap(false, Ordering::AcqRel) {
            self.mark_dirty();
            return;
        }
        let mut stderr = std::io::stderr().lock();
        use std::io::Write;
        let _ = writeln!(stderr, "on-main-thread");
        let _ = stderr.flush();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Crash,
    /// Null store inside `process`, so the helper sees `SIGSEGV`.
    Segfault,
    Hang,
    Callback,
    Restart,
    /// Writes `note-ons + note-offs * 16` and keeps the totals across blocks.
    Count,
    /// Writes the key of the latest note-off, or 0 when none has arrived.
    Key,
    Activate,
    Process,
    Generate,
    /// One-voice release/delay fixture; cleared only by CLAP reset.
    Tail,
    /// Declares three note ports; only MIDI all-sounds-off clears its tail.
    TailLegacy,
    /// Sample-accurate stereo attack fixture for transparent note boundaries.
    Onset,
    /// Sleeps in every `process`, slower than real time at the host's blocks.
    SlowProcess,
    /// The first state save outlasts the host's 2 s state wait.
    SlowSave,
    /// Reports a different latency after each activation.
    Latency,
    /// A poisoned state: NaN from frame 10 of the second block until reset.
    Nan,
    /// Saves and loads a state longer than one control frame's body.
    LargeState,
    /// Delays its main audio by the latency it reports. See [`delay`].
    Delay,
    /// Writes a finite 2.0e38 to every output: far too loud, but neither a
    /// NaN nor an infinity.
    Huge,
    /// Reports a latency and delays nothing. See [`latency::claim`].
    Claim,
    /// Requests a restart in its first block, and a second one while the
    /// host is still inside that restart's activation.
    RestartNested,
}

/// For [`Fault::RestartNested`]: activations so far, and whether the first
/// block already asked. One helper owns one instance.
static NESTED_ACTIVATIONS: AtomicU8 = AtomicU8::new(0);
static NESTED_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn fault_code(text: &str) -> u8 {
    match text {
        "crash" => 1,
        "hang" => 2,
        "segfault" => 13,
        "callback" => 3,
        "restart" => 4,
        "count" => 5,
        "key" => 6,
        "activate" => 7,
        "process" => 8,
        "generate" => 9,
        "tail" => 10,
        "tail-legacy" => 11,
        "onset" => 12,
        "slow-save" => 14,
        "slow-process" => 15,
        "latency" => 16,
        "nan" => 17,
        "large-state" => 18,
        "huge" => 20,
        "restart-nested" => 22,
        _ if text.starts_with("delay:") => 19,
        // The event counter, with the second process call held as `process-wait`.
        _ if text.starts_with("count-wait:") => 5,
        _ if text.starts_with("claim:") => 21,
        _ => 0,
    }
}

fn fault_from_code(code: u8) -> Fault {
    match code {
        1 => Fault::Crash,
        2 => Fault::Hang,
        13 => Fault::Segfault,
        3 => Fault::Callback,
        4 => Fault::Restart,
        5 => Fault::Count,
        6 => Fault::Key,
        7 => Fault::Activate,
        8 => Fault::Process,
        9 => Fault::Generate,
        10 => Fault::Tail,
        11 => Fault::TailLegacy,
        12 => Fault::Onset,
        14 => Fault::SlowSave,
        15 => Fault::SlowProcess,
        16 => Fault::Latency,
        17 => Fault::Nan,
        18 => Fault::LargeState,
        19 => Fault::Delay,
        20 => Fault::Huge,
        21 => Fault::Claim,
        22 => Fault::RestartNested,
        _ => Fault::None,
    }
}

fn fault_now() -> Fault {
    fault_from_code(FAULT_BITS.load(Ordering::Relaxed))
}

/// `ECHODE_TEST_FAULT` wins. A file `echode-pro07-<clap id>` in the temp
/// directory selects the fault for one plugin when the product route has none.
fn note_fault(plugin_id: &CStr) {
    let from_env = std::env::var("ECHODE_TEST_FAULT").ok();
    let from_file = plugin_id.to_str().ok().and_then(|id| {
        std::fs::read_to_string(std::env::temp_dir().join(format!("echode-pro07-{id}"))).ok()
    });
    let text = from_env.or(from_file).unwrap_or_default();
    if let Some(path) = text.trim().strip_prefix("prepare-wait:") {
        use std::io::{Read, Write};
        let mut socket = std::os::unix::net::UnixStream::connect(path).expect("prepare handshake");
        socket.write_all(&[1]).expect("prepare reached");
        let mut release = [0];
        socket.read_exact(&mut release).expect("prepare released");
    }
    if let Some(path) = text.trim().strip_prefix("params-wait:") {
        params::hold_listings(path);
    }
    if let Some(path) = text.trim().strip_prefix("gui:") {
        gui::remote_at(path);
    }
    if let Some(lengths) = text.trim().strip_prefix("delay:") {
        delay::configure(lengths);
    }
    if let Some(frames) = text.trim().strip_prefix("claim:") {
        latency::claim(frames);
    }
    FAULT_BITS.store(fault_code(text.trim()), Ordering::Relaxed);
}

/// One test voice, with a 32-sample attack and 64-sample period. These lengths
/// describe this synthetic input only; the host has no voice or timing cap.
#[derive(Clone, Copy, Default)]
struct Onset {
    held: bool,
    age: u64,
}

impl Onset {
    fn render(
        &mut self,
        output: &mut [f32],
        events: &clack_plugin::events::io::InputEvents<'_>,
        scale: f32,
    ) {
        let mut next = 0usize;
        for (frame, sample) in output.iter_mut().enumerate() {
            while next < events.len() as usize && events[next].header().time() as usize <= frame {
                use clack_plugin::events::spaces::CoreEventSpace;
                match events[next].as_core_event() {
                    Some(CoreEventSpace::NoteOn(_)) => {
                        self.held = true;
                        self.age = 0;
                    }
                    Some(CoreEventSpace::NoteOff(_) | CoreEventSpace::NoteChoke(_)) => {
                        self.held = false;
                    }
                    _ => {}
                }
                next += 1;
            }
            *sample = if self.held {
                let ramp = self.age.min(32) as f32 / 32.0;
                let triangle = 1.0 - ((self.age % 64) as f32 / 16.0 - 2.0).abs();
                self.age += 1;
                scale * triangle * ramp
            } else {
                0.0
            };
        }
    }
}

pub struct TestGainProcessor<'a> {
    fault: Fault,
    role: Role,
    host: HostSharedHandle<'a>,
    seen_ons: u32,
    seen_offs: u32,
    /// Note id from the note-on. `u32::MAX` until one arrives.
    on_id: u32,
    /// Key of a note-off whose note id matches `on_id`.
    off_key: u8,
    tail_gate: bool,
    tail_remaining: usize,
    onset: Onset,
    // A deliberately misbehaving third-party reset for the parent's lifecycle
    // regression. Socket setup is off RT; only this fault blocks in reset.
    reset_gate: Option<std::os::unix::net::UnixStream>,
    reset_gate_used: bool,
    // Off-RT connection; this deliberately broken plugin holds its second block.
    process_gate: Option<std::os::unix::net::UnixStream>,
    process_gate_ready: bool,
    /// For [`Fault::Nan`]: blocks processed, and whether a reset cleared it.
    nan_blocks: u32,
    nan_cleared: bool,
    /// For [`Fault::Delay`].
    delay: Option<delay::Line>,
}

impl<'a> PluginAudioProcessor<'a, (), TestMain> for TestGainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        main_thread: &TestMain,
        _shared: &'a (),
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        if fault_now() == Fault::Latency {
            latency::activated(&main_thread.host);
        }
        if fault_now() == Fault::Claim {
            latency::report(latency::claimed());
        }
        let delay = (fault_now() == Fault::Delay).then(|| {
            let frames = delay::activated();
            latency::report(frames);
            delay::Line::new(frames, audio_config.max_frames_count)
        });
        let fault = fault_now();
        if fault == Fault::Activate {
            return Err(PluginError::Message("activate refused"));
        }
        // The second activation is the one the first block's restart runs.
        if fault == Fault::RestartNested {
            let activation = NESTED_ACTIVATIONS.fetch_add(1, Ordering::AcqRel) + 1;
            // Main thread, so the line may allocate; the test counts restarts by it.
            eprintln!("nested-activation {activation}");
            if activation == 2 {
                _host.shared().request_restart();
                // An activation that takes a while after asking, as one that
                // loads resources does. The host's processing thread sees the
                // request before this activation returns.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Ok(Self {
            fault,
            role: main_thread.role,
            host: _host.shared(),
            seen_ons: 0,
            seen_offs: 0,
            on_id: u32::MAX,
            off_key: 0,
            tail_gate: false,
            tail_remaining: 0,
            onset: Onset::default(),
            reset_gate: std::env::var("ECHODE_TEST_FAULT")
                .ok()
                .and_then(|fault| fault.strip_prefix("reset-wait:").map(str::to_owned))
                .map(|path| std::os::unix::net::UnixStream::connect(path).expect("reset gate")),
            reset_gate_used: false,
            process_gate: std::env::var("ECHODE_TEST_FAULT")
                .ok()
                .and_then(|fault| {
                    fault
                        .strip_prefix("process-wait:")
                        .or_else(|| fault.strip_prefix("count-wait:"))
                        .map(str::to_owned)
                })
                .map(|path| {
                    let gate = std::os::unix::net::UnixStream::connect(path).expect("process gate");
                    gate.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                        .expect("process gate timeout");
                    gate
                }),
            process_gate_ready: false,
            nan_blocks: 0,
            nan_cleared: false,
            delay,
        })
    }

    fn reset(&mut self) {
        self.onset = Onset::default();
        if let Some(line) = &mut self.delay {
            line.reset();
        }
        self.nan_cleared = self.nan_blocks > 1;
        if !self.reset_gate_used
            && let Some(gate) = &mut self.reset_gate
        {
            use std::io::{Read, Write};
            self.reset_gate_used = true;
            gate.write_all(&[1]).expect("reset reached");
            let mut release = [0];
            gate.read_exact(&mut release).expect("reset release");
            gate.write_all(&[2]).expect("reset finished");
        }
        if self.fault != Fault::TailLegacy {
            self.tail_gate = false;
            self.tail_remaining = 0;
        }
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        if self.process_gate_ready
            && let Some(mut gate) = self.process_gate.take()
        {
            use std::io::{Read, Write};
            gate.write_all(&[1]).expect("process reached");
            let mut release = [0];
            gate.read_exact(&mut release).expect("process release");
            gate.write_all(&[2]).expect("process finished");
        }
        self.process_gate_ready = true;
        match self.fault {
            Fault::Process => return Err(PluginError::Message("process refused")),
            Fault::Crash => std::process::abort(),
            Fault::Segfault => loop {
                // A real fault, not `raise`. The handler must run if the kernel
                // turns this into `SIGSEGV`. Do not fall through to `abort`.
                // SAFETY: none: writing to address zero is the deliberate fault of the `segfault`
                // fixture, so the kernel raises a real SIGSEGV; execution never continues past it.
                unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 1) };
            },
            Fault::Hang => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            // Several times one 256-frame block at 48 kHz (about 5.3 ms).
            Fault::SlowProcess => std::thread::sleep(std::time::Duration::from_millis(20)),
            Fault::Callback => self.host.request_callback(),
            Fault::Restart | Fault::Latency => self.host.request_restart(),
            Fault::RestartNested => {
                if !NESTED_REQUESTED.swap(true, Ordering::AcqRel) {
                    self.host.request_restart();
                }
            }
            Fault::Delay => {
                if let Some(line) = &mut self.delay {
                    line.process(&mut audio, events.input)?;
                }
                if delay::restart_due() {
                    self.host.request_restart();
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Onset => {
                let initial = self.onset;
                let mut channel = 0usize;
                for mut port in &mut audio {
                    let Some(channels) = port.channels()?.into_f32() else {
                        continue;
                    };
                    for pair in channels {
                        let output = match pair {
                            ChannelPair::InputOnly(_) => continue,
                            ChannelPair::OutputOnly(output)
                            | ChannelPair::InputOutput(_, output)
                            | ChannelPair::InPlace(output) => output,
                        };
                        let mut state = initial;
                        state.render(
                            output,
                            events.input,
                            if channel == 0 { 0.25 } else { -0.125 },
                        );
                        self.onset = state;
                        channel += 1;
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Tail | Fault::TailLegacy => {
                // This fixture's finite release is deliberately longer than a
                // block. It is not a limit on product voices or tail duration.
                const TAIL: usize = 4096;
                for index in 0..events.input.len() as usize {
                    match events.input[index].as_core_event() {
                        Some(clack_plugin::events::spaces::CoreEventSpace::NoteOn(_)) => {
                            self.tail_gate = true;
                        }
                        Some(clack_plugin::events::spaces::CoreEventSpace::NoteOff(_))
                            if self.tail_gate =>
                        {
                            self.tail_gate = false;
                            self.tail_remaining = TAIL;
                        }
                        Some(clack_plugin::events::spaces::CoreEventSpace::Midi(midi))
                            if midi.port_index() == 2
                                && midi.data()[0] & 0xF0 == 0xB0
                                && midi.data()[1] == 120 =>
                        {
                            self.tail_gate = false;
                            self.tail_remaining = 0;
                        }
                        _ => {}
                    }
                }
                let frames = audio.frames_count() as usize;
                for mut port in &mut audio {
                    let Some(channels) = port.channels()?.into_f32() else {
                        continue;
                    };
                    for channel in channels {
                        match channel {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(output) => {
                                for (index, sample) in output.iter_mut().enumerate() {
                                    *sample = if self.tail_gate {
                                        0.25
                                    } else {
                                        0.25 * self.tail_remaining.saturating_sub(index) as f32
                                            / TAIL as f32
                                    };
                                }
                            }
                            ChannelPair::InputOutput(input, output) => {
                                // Remember nonzero input as an effect's delay.
                                if input.iter().any(|sample| *sample != 0.0) {
                                    self.tail_remaining = TAIL;
                                }
                                for (index, (input, output)) in input.iter().zip(output).enumerate()
                                {
                                    *output = *input
                                        + 0.125 * self.tail_remaining.saturating_sub(index) as f32
                                            / TAIL as f32;
                                }
                            }
                            ChannelPair::InPlace(output) => {
                                if output.iter().any(|sample| *sample != 0.0) {
                                    self.tail_remaining = TAIL;
                                }
                                for (index, sample) in output.iter_mut().enumerate() {
                                    *sample += 0.125
                                        * self.tail_remaining.saturating_sub(index) as f32
                                        / TAIL as f32;
                                }
                            }
                        }
                    }
                }
                self.tail_remaining = self.tail_remaining.saturating_sub(frames);
                return Ok(ProcessStatus::Continue);
            }
            Fault::Count => {
                for index in 0..events.input.len() as usize {
                    let Some(core) = events.input[index].as_core_event() else {
                        continue;
                    };
                    match core {
                        clack_plugin::events::spaces::CoreEventSpace::NoteOn(_) => {
                            self.seen_ons = self.seen_ons.saturating_add(1);
                        }
                        clack_plugin::events::spaces::CoreEventSpace::NoteOff(_) => {
                            self.seen_offs = self.seen_offs.saturating_add(1);
                        }
                        _ => {}
                    }
                }
                let value = self.seen_ons as f32 + self.seen_offs as f32 * 16.0;
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        match channel_pair {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(buffer) => buffer.fill(value),
                            ChannelPair::InputOutput(_, output) => output.fill(value),
                            ChannelPair::InPlace(buffer) => buffer.fill(value),
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Key => {
                for index in 0..events.input.len() as usize {
                    let Some(core) = events.input[index].as_core_event() else {
                        continue;
                    };
                    match core {
                        clack_plugin::events::spaces::CoreEventSpace::NoteOn(on) => {
                            if let clack_plugin::events::Match::Specific(id) = on.note_id() {
                                self.on_id = id;
                            }
                        }
                        clack_plugin::events::spaces::CoreEventSpace::NoteOff(off) => {
                            let Some(id) = off.note_id().into_specific() else {
                                continue;
                            };
                            if id != self.on_id {
                                continue;
                            }
                            self.off_key = off
                                .key()
                                .into_specific()
                                .and_then(|key| u8::try_from(key).ok())
                                .unwrap_or(0);
                        }
                        _ => {}
                    }
                }
                let value = f32::from(self.off_key);
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        match channel_pair {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(buffer) => buffer.fill(value),
                            ChannelPair::InputOutput(_, output) => output.fill(value),
                            ChannelPair::InPlace(buffer) => buffer.fill(value),
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Huge => {
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        match channel_pair {
                            ChannelPair::InputOnly(_) => {}
                            ChannelPair::OutputOnly(buffer)
                            | ChannelPair::InputOutput(_, buffer)
                            | ChannelPair::InPlace(buffer) => buffer.fill(2.0e38),
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::Nan => {
                self.nan_blocks = self.nan_blocks.saturating_add(1);
                let poisoned = !self.nan_cleared && self.nan_blocks > 1;
                for mut port_pair in &mut audio {
                    let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                        continue;
                    };
                    for channel_pair in channel_pairs {
                        let buffer = match channel_pair {
                            ChannelPair::InputOnly(_) => continue,
                            ChannelPair::OutputOnly(buffer) => buffer,
                            ChannelPair::InputOutput(_, output) => output,
                            ChannelPair::InPlace(buffer) => buffer,
                        };
                        buffer.fill(0.5);
                        if poisoned && let Some(rest) = buffer.get_mut(10..) {
                            rest.fill(f32::NAN);
                        }
                    }
                }
                return Ok(ProcessStatus::Continue);
            }
            Fault::None
            | Fault::Activate
            | Fault::Generate
            | Fault::SlowSave
            | Fault::LargeState
            | Fault::Claim => {}
        }

        if self.role == Role::State {
            fill_outputs(&mut audio, state_level());
            return Ok(ProcessStatus::Continue);
        }
        if self.role == Role::Params {
            params::process(&mut audio, events.input)?;
            gui::emit(events.output);
            return Ok(ProcessStatus::Continue);
        }
        if self.role == Role::Order {
            let mut last = 0u32;
            let mut ons = 0u32;
            for index in 0..events.input.len() as usize {
                let header = events.input[index].header();
                if header.time() < last {
                    return Err(PluginError::Message("events are out of order"));
                }
                last = header.time();
                if let Some(core) = events.input[index].as_core_event()
                    && matches!(
                        core,
                        clack_plugin::events::spaces::CoreEventSpace::NoteOn(_)
                    )
                {
                    ons = ons.saturating_add(1);
                }
            }
            fill_outputs(&mut audio, ons as f32);
            return Ok(ProcessStatus::Continue);
        }
        if self.role == Role::Mono || (self.role == Role::Flip && port_flip_active()) {
            fill_outputs(&mut audio, 0.25);
            return Ok(ProcessStatus::Continue);
        }
        if self.role == Role::Ports {
            for (port_index, mut port_pair) in (&mut audio).into_iter().enumerate() {
                let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                    continue;
                };
                let constant = if port_index == 0 { None } else { Some(0.125) };
                for channel_pair in channel_pairs {
                    match channel_pair {
                        ChannelPair::InputOnly(_) => {}
                        ChannelPair::OutputOnly(buffer) => {
                            buffer.fill(constant.unwrap_or(0.25));
                        }
                        ChannelPair::InputOutput(input, output) => {
                            for (input, output) in input.iter().zip(output) {
                                *output = constant.unwrap_or(input * 2.0);
                            }
                        }
                        ChannelPair::InPlace(buffer) => {
                            for sample in buffer {
                                *sample = constant.unwrap_or(*sample * 2.0);
                            }
                        }
                    }
                }
            }
            return Ok(ProcessStatus::Continue);
        }

        for mut port_pair in &mut audio {
            let Some(channel_pairs) = port_pair.channels()?.into_f32() else {
                continue;
            };
            for channel_pair in channel_pairs {
                match channel_pair {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(buffer) => buffer.fill(0.0),
                    ChannelPair::InputOutput(input, output) => {
                        for (input, output) in input.iter().zip(output) {
                            *output = input * 2.0;
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for sample in buffer {
                            *sample *= 2.0;
                        }
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}

/// The gain effect, plus an instrument that runs the same processor.
///
/// The instrument id exists so a project can hold both roles. The processor
/// still gains audio by 2; an instrument receives silence and notes, and
/// placed audio is added after it.
struct TestEntry {
    factory: PluginFactoryWrapper<TestFactory>,
}

impl Entry for TestEntry {
    fn new(_plugin_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        // A bundle whose loading never returns, for the scan helper's orphan test.
        if std::env::var("ECHODE_TEST_FAULT").is_ok_and(|fault| fault == "entry-hang") {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
        Ok(Self {
            factory: PluginFactoryWrapper::new(TestFactory::new()),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

fn state_level() -> f32 {
    match STATE_BYTE.load(Ordering::Acquire) {
        64 => 0.25,
        128 => 0.5,
        _ => 0.0,
    }
}

fn fill_outputs(audio: &mut Audio<'_>, value: f32) {
    for mut port_pair in audio {
        let Ok(channels) = port_pair.channels() else {
            continue;
        };
        let Some(channel_pairs) = channels.into_f32() else {
            continue;
        };
        for channel_pair in channel_pairs {
            match channel_pair {
                ChannelPair::InputOnly(_) => {}
                ChannelPair::OutputOnly(buffer) => buffer.fill(value),
                ChannelPair::InputOutput(_, output) => output.fill(value),
                ChannelPair::InPlace(buffer) => buffer.fill(value),
            }
        }
    }
}

struct TestFactory {
    descriptors: Vec<PluginDescriptor>,
}

impl TestFactory {
    fn new() -> Self {
        Self {
            descriptors: vec![
                TestGain::get_descriptor(),
                described(
                    "dev.echode.test-instrument",
                    "Echode Test Instrument",
                    c"instrument",
                ),
                described("dev.echode.test-mono", "Echode Test Mono", c"instrument"),
                described(
                    "dev.echode.test-monogain",
                    "Echode Test Mono Gain",
                    c"audio-effect",
                ),
                described(
                    "dev.echode.test-noports",
                    "Echode Test No Ports",
                    c"audio-effect",
                ),
                described(
                    "dev.echode.test-ports",
                    "Echode Test Ports",
                    c"audio-effect",
                ),
                described(
                    "dev.echode.test-state",
                    "Echode Test State",
                    c"audio-effect",
                ),
                described("dev.echode.test-zflip", "Echode Test Flip", c"audio-effect"),
                described("dev.echode.test-order", "Echode Test Order", c"instrument"),
                described(
                    "dev.echode.test-params",
                    "Echode Test Params",
                    c"audio-effect",
                ),
            ],
        }
    }
}

fn described(id: &str, name: &str, feature: &'static CStr) -> PluginDescriptor {
    PluginDescriptor::new(id, name)
        .with_vendor("Echode")
        .with_version("0.0.0")
        .with_url("https://echode.invalid")
        .with_features([feature])
}

fn role_of(id: &CStr) -> Option<Role> {
    match id.to_bytes() {
        b"dev.echode.test-gain" => Some(Role::Gain),
        b"dev.echode.test-instrument" => Some(Role::Instrument),
        b"dev.echode.test-mono" => Some(Role::Mono),
        b"dev.echode.test-monogain" => Some(Role::MonoGain),
        b"dev.echode.test-noports" => Some(Role::NoPorts),
        b"dev.echode.test-ports" => Some(Role::Ports),
        b"dev.echode.test-state" => Some(Role::State),
        b"dev.echode.test-zflip" => Some(Role::Flip),
        b"dev.echode.test-order" => Some(Role::Order),
        b"dev.echode.test-params" => Some(Role::Params),
        _ => None,
    }
}

impl PluginFactoryImpl for TestFactory {
    fn plugin_count(&self) -> u32 {
        self.descriptors.len() as u32
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        self.descriptors.get(index as usize)
    }

    fn create_plugin<'a>(
        &'a self,
        host_info: HostInfo<'a>,
        plugin_id: &CStr,
    ) -> Option<PluginInstance<'a>> {
        let role = role_of(plugin_id)?;
        let descriptor = self
            .descriptors
            .iter()
            .find(|descriptor| descriptor.id() == Some(plugin_id))?;
        set_role(role);
        note_fault(plugin_id);
        if fault_now() == Fault::Generate {
            return None;
        }
        Some(PluginInstance::new::<TestGain>(
            host_info,
            descriptor,
            TestGain::new_shared,
            TestGain::new_main_thread,
        ))
    }
}

#[derive(Copy, Clone)]
struct PluginAudioPorts;

// SAFETY: `CLAP_EXT_AUDIO_PORTS` is the official identifier of the extension this type stands for.
unsafe impl Extension for PluginAudioPorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_AUDIO_PORTS];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_audio_ports`, the `#[repr(C)]` table CLAP
// declares for that identifier.
unsafe impl ExtensionImplementation<TestGain> for PluginAudioPorts {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_audio_ports {
            count: Some(port_count),
            get: Some(port_get),
        });
}

unsafe extern "C" fn port_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    // SAFETY: the host calls with the plugin pointer this plugin gave it, on the main thread.
    unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
            let (inputs, outputs) = wrapper.main_thread().ports();
            let count = if is_input {
                inputs.len()
            } else {
                outputs.len()
            };
            Ok(u32::try_from(count).unwrap_or(u32::MAX))
        })
    }
    .unwrap_or(0)
}

unsafe extern "C" fn port_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    // SAFETY: the host calls with the plugin pointer this plugin gave it, on the main thread, and
    // writable storage for one descriptor in `info`.
    unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
            let (inputs, outputs) = wrapper.main_thread().ports();
            let counts = if is_input { inputs } else { outputs };
            let Some(channels) = counts.get(index as usize).copied() else {
                return Ok(false);
            };
            *info = clap_audio_port_info {
                id: index,
                name: [0; clap_sys::string_sizes::CLAP_NAME_SIZE],
                flags: if index == 0 {
                    CLAP_AUDIO_PORT_IS_MAIN
                } else {
                    0
                },
                channel_count: channels,
                port_type: if channels == 1 {
                    CLAP_PORT_MONO.as_ptr()
                } else {
                    CLAP_PORT_STEREO.as_ptr()
                },
                in_place_pair: u32::MAX,
            };
            Ok(true)
        })
    }
    .unwrap_or(false)
}

#[derive(Copy, Clone)]
struct SeenHostState(RawExtension<HostExtensionSide, clap_host_state>);

// SAFETY: `CLAP_EXT_STATE` is the official identifier of the extension this type stands for.
unsafe impl Extension for SeenHostState {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_STATE];
    type ExtensionSide = HostExtensionSide;

    unsafe fn from_raw(raw: RawExtension<Self::ExtensionSide>) -> Self {
        // SAFETY: `IDENTIFIERS` names the state extension, whose host table is `clap_host_state`,
        // so the raw extension clack found has this type.
        Self(unsafe { raw.cast() })
    }
}

impl SeenHostState {
    fn mark_dirty(self) {
        let host = self.0.host_ptr();
        // SAFETY: the host's `clap_host_state` table stays valid while the plugin lives.
        let mark = unsafe { self.0.as_ptr().as_ref().mark_dirty };
        if let Some(mark) = mark {
            // SAFETY: `mark_dirty` is the host's own function, called with the host pointer it gave
            // this plugin.
            unsafe { mark(host.as_ptr()) };
        }
    }
}

#[derive(Copy, Clone)]
struct PluginStateExt;

// SAFETY: `CLAP_EXT_STATE` is the official identifier of the extension this type stands for.
unsafe impl Extension for PluginStateExt {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_STATE];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_state`, the `#[repr(C)]` table CLAP declares
// for that identifier.
unsafe impl ExtensionImplementation<TestGain> for PluginStateExt {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_state {
            save: Some(state_save),
            load: Some(state_load),
        });
}

/// The `large-state` fixture's length: one control frame's largest body
/// (64 MiB less the type and request id) plus 1 MiB, so the state crosses the
/// socket in two frames. It sizes this fixture only; states are not capped.
const LARGE_STATE: usize = 64 * 1024 * 1024 - 12 + 1024 * 1024;

/// The chunk the `large-state` fixture writes and reads per stream call.
const LARGE_STATE_CHUNK: usize = 1024 * 1024;

/// Set once the `slow-save` fault has delayed a save.
static SLOW_SAVE_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let _ = plugin;
    if stream.is_null() {
        return false;
    }
    if fault_now() == Fault::SlowSave && !SLOW_SAVE_DONE.swap(true, Ordering::AcqRel) {
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    if current_role() == Role::Params {
        let bytes = params::save();
        // SAFETY: `stream` is the host's non-null output stream for this save, and `bytes` is
        // readable for its length.
        return unsafe {
            let Some(write) = (*stream).write else {
                return false;
            };
            write(stream, bytes.as_ptr().cast::<c_void>(), bytes.len() as u64) == bytes.len() as i64
        };
    }
    let byte = STATE_BYTE.load(Ordering::Acquire);
    if fault_now() == Fault::LargeState {
        // SAFETY: `stream` is the host's non-null output stream for this save.
        return unsafe { save_large_state(stream, byte) };
    }
    // SAFETY: `stream` is the host's non-null output stream for this save, and `byte` is readable.
    unsafe {
        let Some(write) = (*stream).write else {
            return false;
        };
        write(stream, std::ptr::from_ref(&byte).cast::<c_void>(), 1) == 1
    }
}

/// Writes `byte` [`LARGE_STATE`] times in [`LARGE_STATE_CHUNK`] calls.
///
/// # Safety
/// `stream` is the host's non-null output stream for this save.
unsafe fn save_large_state(stream: *const clap_ostream, byte: u8) -> bool {
    // SAFETY: the caller passes the host's non-null output stream.
    let Some(write) = (unsafe { (*stream).write }) else {
        return false;
    };
    let chunk = vec![byte; LARGE_STATE_CHUNK];
    let mut left = LARGE_STATE;
    while left > 0 {
        let count = left.min(chunk.len());
        // SAFETY: `write` is the stream's own function, given the stream and `count` readable bytes
        // of `chunk`.
        let written = unsafe { write(stream, chunk.as_ptr().cast::<c_void>(), count as u64) };
        if usize::try_from(written).ok() != Some(count) {
            return false;
        }
        left -= count;
    }
    true
}

/// Reads a whole `large-state` save: [`LARGE_STATE`] copies of one byte.
///
/// # Safety
/// `stream` is the host's non-null input stream for this load.
unsafe fn load_large_state(stream: *const clap_istream) -> Option<u8> {
    // SAFETY: the caller passes the host's non-null input stream.
    let read = unsafe { (*stream).read }?;
    let mut chunk = vec![0u8; LARGE_STATE_CHUNK];
    let mut first = None;
    let mut total = 0usize;
    loop {
        // SAFETY: `read` is the stream's own function, given the stream and `chunk`, writable for
        // its length.
        let count = unsafe {
            read(
                stream,
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk.len() as u64,
            )
        };
        let count = usize::try_from(count).ok()?;
        if count == 0 {
            break;
        }
        let bytes = chunk.get(..count)?;
        let byte = *first.get_or_insert(*bytes.first()?);
        if bytes.iter().any(|value| *value != byte) {
            return None;
        }
        total += count;
    }
    (total == LARGE_STATE).then_some(first?)
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    if stream.is_null() {
        return false;
    }
    if current_role() == Role::Params {
        // One byte more than the fixture writes, so trailing bytes are refused.
        let mut bytes = [0u8; 17];
        // SAFETY: `stream` is the host's non-null input stream for this load, and `bytes` is
        // writable for its length.
        let read = unsafe {
            let Some(read) = (*stream).read else {
                return false;
            };
            read(
                stream,
                bytes.as_mut_ptr().cast::<c_void>(),
                bytes.len() as u64,
            )
        };
        return usize::try_from(read).is_ok_and(|read| params::load(&bytes[..read]));
    }
    if fault_now() == Fault::LargeState {
        // SAFETY: `stream` is the host's non-null input stream for this load.
        let Some(byte) = (unsafe { load_large_state(stream) }) else {
            return false;
        };
        STATE_BYTE.store(byte, Ordering::Release);
        // Like a short load, so a test can start a background save of the
        // large state through the dirty notice.
        // SAFETY: the host calls with the plugin pointer this plugin gave it, on the main thread.
        unsafe {
            PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
                wrapper.main_thread().mark_dirty();
                Ok(())
            });
        }
        return true;
    }
    let mut byte = 0u8;
    let mut extra = 0u8;
    // SAFETY: `stream` is the host's non-null input stream for this load, and each byte is
    // writable.
    let accepted = unsafe {
        let Some(read) = (*stream).read else {
            return false;
        };
        if read(stream, std::ptr::from_mut(&mut byte).cast::<c_void>(), 1) != 1 {
            return false;
        }
        read(stream, std::ptr::from_mut(&mut extra).cast::<c_void>(), 1) == 0
    };
    if !accepted {
        return false;
    }
    STATE_BYTE.store(byte, Ordering::Release);
    // SAFETY: the host calls with the plugin pointer this plugin gave it, on the main thread.
    unsafe {
        PluginWrapper::<TestGain>::handle(plugin, |wrapper| {
            wrapper.main_thread().mark_dirty();
            Ok(())
        });
    }
    true
}

#[derive(Copy, Clone)]
struct PluginNotePorts;

// SAFETY: `CLAP_EXT_NOTE_PORTS` is the official identifier of the extension this type stands for.
unsafe impl Extension for PluginNotePorts {
    const IDENTIFIERS: &[&CStr] = &[CLAP_EXT_NOTE_PORTS];
    type ExtensionSide = PluginExtensionSide;

    unsafe fn from_raw(_raw: RawExtension<Self::ExtensionSide>) -> Self {
        Self
    }
}

// SAFETY: `IMPLEMENTATION` points at a `clap_plugin_note_ports`, the `#[repr(C)]` table CLAP
// declares for that identifier.
unsafe impl ExtensionImplementation<TestGain> for PluginNotePorts {
    const IMPLEMENTATION: RawExtensionImplementation =
        RawExtensionImplementation::new(&clap_plugin_note_ports {
            count: Some(note_port_count),
            get: Some(note_port_get),
        });
}

unsafe extern "C" fn note_port_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    // Three ports only exercise this fixture's declaration and final port.
    if fault_now() == Fault::TailLegacy {
        if is_input { 3 } else { 0 }
    } else {
        u32::from(is_input)
    }
}

unsafe extern "C" fn note_port_get(
    _plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    // SAFETY: `note_port_count` only reads the fault setting and never dereferences the plugin
    // pointer.
    if info.is_null() || index >= unsafe { note_port_count(_plugin, is_input) } || !is_input {
        return false;
    }
    // SAFETY: `info` is non-null, checked above, and the host gives writable storage for one
    // descriptor.
    unsafe {
        *info = clap_note_port_info {
            id: index,
            supported_dialects: CLAP_NOTE_DIALECT_CLAP
                | if fault_now() == Fault::TailLegacy {
                    CLAP_NOTE_DIALECT_MIDI
                } else {
                    0
                },
            preferred_dialect: CLAP_NOTE_DIALECT_CLAP,
            name: [0; 256],
        };
    }
    true
}

clack_export_entry!(TestEntry);
