//! The shared memory between the host and one helper (`MappedRegion`).
//!
//! A plugin in the helper can write any bytes anywhere in the mapping, at any
//! time. This target maps the region a second time, the way the helper does,
//! and interleaves such writes with what the host does: submit blocks, collect
//! them, and read the header's flags.
//!
//! Input: five layout bytes, then operations. Each operation is one byte and
//! its operands; see [`Op`].

use std::os::fd::{AsRawFd, OwnedFd};

use echode_realtime::{Event, Target as EventTarget, Transport};
use echode_sandbox::{Collected, Delivered, MappedRegion, SessionLayout};

use crate::bytes::Bytes;

/// More operations are ignored. Bounds this target's run time only.
const MAX_OPS: usize = 64;

enum Op {
    /// The helper writes bytes at an offset (modulo the mapping length).
    Write,
    /// The host submits a block of frames with events.
    Submit,
    /// The host collects one of its submitted generations into slices.
    Collect,
    /// The host collects one of its submitted generations into plane buffers.
    CollectPlanes,
    /// The host reads the header's counters and flags.
    Flags,
    /// The helper echoes routed input events and copies the input events.
    HelperReads,
}

impl Op {
    fn from_byte(byte: u8) -> Self {
        match byte % 6 {
            0 => Self::Write,
            1 => Self::Submit,
            2 => Self::Collect,
            3 => Self::CollectPlanes,
            4 => Self::Flags,
            _ => Self::HelperReads,
        }
    }
}

pub(crate) fn run(data: &[u8]) {
    let mut input = Bytes::new(data);
    let layout = SessionLayout {
        max_frames: 1 + u32::from(input.u8() % 64),
        input_channels: u32::from(input.u8() % 3),
        output_channels: u32::from(input.u8() % 3),
        input_events: u32::from(input.u8() % 5),
        output_events: u32::from(input.u8() % 5),
    };
    if layout.validate().is_err() {
        return;
    }
    let host = MappedRegion::create_shared(layout).expect("create the shared region");
    let helper = HelperView::map(host.duplicate_fd().expect("duplicate the region"));
    let mut generations: Vec<u64> = Vec::new();
    for _ in 0..MAX_OPS {
        if input.is_empty() {
            break;
        }
        match Op::from_byte(input.u8()) {
            Op::Write => {
                let offset = usize::from(input.u16());
                let len = usize::from(input.u8());
                helper.write(offset, input.take(len));
            }
            Op::Submit => {
                let frames = 1 + usize::from(input.u8()) % layout.max_frames as usize;
                let count = usize::from(input.u8()) % (layout.input_events as usize + 1);
                let planes: Vec<Vec<f32>> = (0..layout.input_channels)
                    .map(|channel| vec![channel as f32 * 0.25; frames])
                    .collect();
                let input_planes: Vec<&[f32]> = planes.iter().map(Vec::as_slice).collect();
                let events: Vec<(Event, u32)> = (0..count)
                    .map(|index| (sample_event(index), index as u32))
                    .collect();
                if let Ok(generation) = host.submit_block(frames, &input_planes, &events) {
                    assert!(generation >= 1, "the host submitted generation 0");
                    generations.push(generation);
                }
            }
            Op::Collect => {
                let Some(generation) = pick(&generations, input.u8()) else {
                    continue;
                };
                let frames = 1 + usize::from(input.u8()) % layout.max_frames as usize;
                let mut planes = vec![vec![0.0f32; frames]; layout.output_channels as usize];
                let mut output: Vec<&mut [f32]> =
                    planes.iter_mut().map(Vec::as_mut_slice).collect();
                let mut delivered = delivered_slots(layout);
                let result = host.collect_ready(generation, &mut output, &mut delivered);
                assert_delivered(result, delivered.len());
            }
            Op::CollectPlanes => {
                let Some(generation) = pick(&generations, input.u8()) else {
                    continue;
                };
                let frames = 1 + usize::from(input.u8()) % layout.max_frames as usize;
                let mut planes =
                    vec![vec![0.0f32; layout.max_frames as usize]; layout.output_channels as usize];
                let mut delivered = delivered_slots(layout);
                let result = host.collect_planes(generation, &mut planes, frames, &mut delivered);
                assert_delivered(result, delivered.len());
            }
            Op::Flags => {
                let _ = (
                    host.published(),
                    host.alarm(),
                    host.process_refused(),
                    host.fatal_notice(),
                    host.is_ready(),
                    host.is_dead(),
                );
            }
            Op::HelperReads => {
                let Some(generation) = pick(&generations, input.u8()) else {
                    continue;
                };
                host.echo_routed(generation, u32::from(input.u8() % 4));
                let mut events = Vec::with_capacity(layout.input_events as usize);
                host.copy_input_events(generation, &mut events);
                assert!(events.len() <= layout.input_events as usize);
            }
        }
    }
}

fn pick(generations: &[u64], byte: u8) -> Option<u64> {
    if generations.is_empty() {
        return None;
    }
    generations
        .get(usize::from(byte) % generations.len())
        .copied()
}

fn delivered_slots(layout: SessionLayout) -> Vec<Delivered> {
    vec![
        Delivered {
            event: sample_event(0),
            device: 0,
        };
        layout.output_events as usize
    ]
}

fn assert_delivered(result: Result<Collected, echode_sandbox::RegionError>, capacity: usize) {
    if let Ok(Collected::Ready { events }) = result {
        assert!(
            events <= capacity,
            "collected {events} events into {capacity} slots"
        );
    }
}

fn sample_event(index: usize) -> Event {
    match index % 3 {
        0 => Event::note_on(index as u32, 1, 40 + index as u32, 60, 0.8),
        1 => Event::NoteOff(echode_realtime::NoteEvent {
            time: index as u32,
            track: 1,
            target: EventTarget::any(),
            velocity: 0.0,
        }),
        _ => Event::Transport(Transport {
            time: index as u32,
            playing: true,
            recording: false,
            loop_active: true,
            tempo_bpm: 120.0,
            tempo_inc: 0.0,
            song_pos_beats: 4.0,
            song_pos_seconds: 2.0,
            loop_start_beats: 0.0,
            loop_end_beats: 8.0,
            loop_start_seconds: 0.0,
            loop_end_seconds: 4.0,
            bar_start: 4.0,
            bar_number: 1,
            signature_numerator: 4,
            signature_denominator: 4,
        }),
    }
}

/// The helper's mapping of the same memory.
struct HelperView {
    base: *mut u8,
    len: usize,
    _fd: OwnedFd,
}

impl HelperView {
    fn map(fd: OwnedFd) -> Self {
        // SAFETY: `stat` is plain data that fstat fills.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is open and `stat` is a live buffer.
        assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) }, 0);
        let len = usize::try_from(stat.st_size).expect("region size");
        // SAFETY: a new shared mapping of the whole object; checked below.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED, "map the region");
        Self {
            base: base.cast(),
            len,
            _fd: fd,
        }
    }

    fn write(&self, offset: usize, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            let at = (offset + index) % self.len;
            // SAFETY: `at` is inside the mapping. A volatile write models
            // another process: the host may be reading the same bytes.
            unsafe { self.base.add(at).write_volatile(*byte) };
        }
    }
}

impl Drop for HelperView {
    fn drop(&mut self) {
        // SAFETY: this view owns the mapping it created.
        unsafe { libc::munmap(self.base.cast(), self.len) };
    }
}

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    // Stereo, 16 frames, 2 events each way. Submit, let the "helper" write a
    // completed counter and an output count, then collect both ways.
    let mut submit_collect = vec![15, 2, 2, 2, 2];
    submit_collect.extend_from_slice(&[1, 15, 2]);
    submit_collect.extend_from_slice(&[0, 16, 0, 8, 1, 0, 0, 0, 0, 0, 0, 0]);
    submit_collect.extend_from_slice(&[2, 0, 15, 3, 0, 15, 4, 5, 0, 3]);
    // Events only, no audio.
    let events_only = vec![7, 0, 0, 4, 4, 1, 7, 4, 2, 0, 7, 5, 0, 1, 4];
    vec![submit_collect, events_only]
}
