//! A real audio delay that reports its length as latency, for the host's delay
//! compensation (ADR 0339). One helper owns one instance.
//!
//! `ECHODE_TEST_FAULT=delay:<frames>` delays the main input by `<frames>`.
//! An instrument, which has no input, delays a 0.25 gate held from each
//! note-on to its note-off instead. `delay:<first>:<then>` asks for one restart
//! after its first block, then delays and reports `<then>`.
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static FIRST: AtomicU32 = AtomicU32::new(0);
/// `u32::MAX` when no restart is asked for.
static THEN: AtomicU32 = AtomicU32::new(u32::MAX);
static ACTIVATED: AtomicBool = AtomicBool::new(false);
static RESTART_ASKED: AtomicBool = AtomicBool::new(false);

/// Reads `<first>[:<then>]`. A malformed length leaves the plugin without delay.
pub(crate) fn configure(lengths: &str) {
    let mut parts = lengths.split(':');
    let first = parts.next().and_then(|text| text.parse().ok()).unwrap_or(0);
    let then = parts
        .next()
        .and_then(|text| text.parse().ok())
        .unwrap_or(u32::MAX);
    FIRST.store(first, Ordering::Relaxed);
    THEN.store(then, Ordering::Relaxed);
}

/// The length for this activation: `<first>`, or `<then>` after a restart.
pub(crate) fn activated() -> u32 {
    let then = THEN.load(Ordering::Relaxed);
    if ACTIVATED.swap(true, Ordering::Relaxed) && then != u32::MAX {
        then
    } else {
        FIRST.load(Ordering::Relaxed)
    }
}

/// True once, after the first block, when a second length was given.
pub(crate) fn restart_due() -> bool {
    THEN.load(Ordering::Relaxed) != u32::MAX && !RESTART_ASKED.swap(true, Ordering::Relaxed)
}

pub(crate) struct Line {
    /// One ring per main channel. Two covers this fixture's stereo ports.
    rings: [Vec<f32>; 2],
    at: usize,
    held: bool,
    gate: Vec<f32>,
}

impl Line {
    /// Activation is off the audio thread; `process` never allocates.
    pub(crate) fn new(frames: u32, max_frames: u32) -> Self {
        let frames = frames as usize;
        Self {
            rings: [vec![0.0; frames], vec![0.0; frames]],
            at: 0,
            held: false,
            gate: vec![0.0; max_frames as usize],
        }
    }

    pub(crate) fn reset(&mut self) {
        for ring in &mut self.rings {
            ring.fill(0.0);
        }
        self.held = false;
    }

    pub(crate) fn process(
        &mut self,
        audio: &mut Audio<'_>,
        events: &InputEvents<'_>,
    ) -> Result<(), PluginError> {
        let frames = (audio.frames_count() as usize).min(self.gate.len());
        let mut from = 0usize;
        for index in 0..events.len() as usize {
            let event = &events[index];
            let to = (event.header().time() as usize).min(frames);
            let level = if self.held { 0.25 } else { 0.0 };
            self.gate
                .iter_mut()
                .take(to)
                .skip(from)
                .for_each(|sample| *sample = level);
            from = from.max(to);
            match event.as_core_event() {
                Some(CoreEventSpace::NoteOn(_)) => self.held = true,
                Some(CoreEventSpace::NoteOff(_)) => self.held = false,
                _ => {}
            }
        }
        let level = if self.held { 0.25 } else { 0.0 };
        self.gate
            .iter_mut()
            .take(frames)
            .skip(from)
            .for_each(|sample| *sample = level);
        let start = self.at;
        let mut channel = 0usize;
        if let Some(mut port) = audio.port_pair(0) {
            let Some(pairs) = port.channels()?.into_f32() else {
                return Ok(());
            };
            for pair in pairs {
                let Some(ring) = self.rings.get_mut(channel) else {
                    break;
                };
                channel += 1;
                let mut at = start;
                let mut step = |input: f32| -> f32 {
                    let Some(slot) = ring.get_mut(at) else {
                        return input;
                    };
                    let output = std::mem::replace(slot, input);
                    at = (at + 1) % ring.len();
                    output
                };
                match pair {
                    ChannelPair::InputOnly(_) => {}
                    ChannelPair::OutputOnly(output) => {
                        for (sample, gate) in output.iter_mut().zip(&self.gate).take(frames) {
                            *sample = step(*gate);
                        }
                    }
                    ChannelPair::InputOutput(input, output) => {
                        for (sample, input) in output.iter_mut().zip(input).take(frames) {
                            *sample = step(*input);
                        }
                    }
                    ChannelPair::InPlace(buffer) => {
                        for sample in buffer.iter_mut().take(frames) {
                            *sample = step(*sample);
                        }
                    }
                }
            }
        }
        let length = self.rings.first().map_or(0, Vec::len);
        if length != 0 {
            self.at = (start + frames) % length;
        }
        Ok(())
    }
}
