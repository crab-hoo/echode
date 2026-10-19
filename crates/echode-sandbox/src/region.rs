//! Two block slots in one locked shared mapping.
//!
//! The helper is untrusted. Counts are checked against the layout copied at
//! attach time, and events are plain records ([`crate::wire`]), never a Rust enum.

use crate::sync::{self, SyncFlags, WaitError};
use crate::wire::{RECORD_BYTES, decode_event, device_index, encode_event, encode_routed};
use crate::{CMD_PROCESS, MAGIC};
use echode_realtime::Event;
use memmap2::MmapMut;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const HEADER_BYTES: usize = 128;
const SLOT_HEAD: usize = 64;
const SLOTS: usize = 2;

static SHM_SEQ: AtomicU64 = AtomicU64::new(0);
static ABORT_REGION: AtomicPtr<MappedRegion> = AtomicPtr::new(ptr::null_mut());
static ABORT_ARMED: AtomicBool = AtomicBool::new(false);

/// Point the process-wide fatal-signal handler at this mapping.
///
/// `SIGABRT`, `SIGSEGV`, `SIGBUS`, and `SIGILL` share it. The handler only
/// stores two atomics, then re-raises the same signal. It does not take a
/// Mach exception port, so a fault that never becomes one of these signals
/// is still noticed when the process exits. Later calls replace the mapping
/// and do not call `sigaction` again.
pub fn arm_abort_notice(region: &MappedRegion) {
    point_abort_notice(region);
    if ABORT_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = abort_notice as *const () as libc::sighandler_t;
        libc::sigemptyset(ptr::from_mut(&mut action.sa_mask));
        for signal in [libc::SIGABRT, libc::SIGSEGV, libc::SIGBUS, libc::SIGILL] {
            let _ = libc::sigaction(signal, &action, ptr::null_mut());
        }
    }
}

/// Replace the mapping the handler writes into. Does not call `sigaction`.
/// The processing thread uses this after the handler is already installed.
pub fn point_abort_notice(region: &MappedRegion) {
    ABORT_REGION.store(ptr::from_ref(region).cast_mut(), Ordering::Release);
}

extern "C" fn abort_notice(signal: i32) {
    let region = ABORT_REGION.load(Ordering::Relaxed);
    if !region.is_null() {
        unsafe { (*region).mark_aborted() };
    }
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Chosen when the session is created. Zero audio channels are legal.
/// A session with no audio and no events is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLayout {
    pub max_frames: u32,
    pub input_channels: u32,
    pub output_channels: u32,
    pub input_events: u32,
    pub output_events: u32,
}

impl SessionLayout {
    /// Stereo, 512 frames, eight events each way. Not a protocol limit.
    pub const fn stereo() -> Self {
        Self {
            max_frames: 512,
            input_channels: 2,
            output_channels: 2,
            input_events: 8,
            output_events: 8,
        }
    }

    pub fn validate(self) -> Result<(), RegionError> {
        if self.max_frames == 0
            || (self.input_channels == 0
                && self.output_channels == 0
                && self.input_events == 0
                && self.output_events == 0)
        {
            return Err(RegionError::BadShape {
                frames: self.max_frames as usize,
                channels: self.input_channels as usize,
            });
        }
        self.mapping_len()?;
        Ok(())
    }

    fn slot_stride(self) -> Result<usize, RegionError> {
        let frames = self.max_frames as usize;
        let channels = (self.input_channels as usize)
            .checked_add(self.output_channels as usize)
            .ok_or(RegionError::BadShape {
                frames,
                channels: 0,
            })?;
        let audio = channels
            .checked_mul(frames)
            .and_then(|n| n.checked_mul(std::mem::size_of::<f32>()))
            .ok_or(RegionError::BadShape { frames, channels })?;
        let events = (self.input_events as usize)
            .checked_add(self.output_events as usize)
            .and_then(|n| n.checked_mul(RECORD_BYTES))
            .ok_or(RegionError::BadShape { frames, channels })?;
        let slot = SLOT_HEAD
            .checked_add(audio)
            .and_then(|n| n.checked_add(events))
            .ok_or(RegionError::BadShape { frames, channels })?;
        align_up(slot, 64).ok_or(RegionError::BadShape { frames, channels })
    }

    fn mapping_len(self) -> Result<usize, RegionError> {
        let frames = self.max_frames as usize;
        let channels = self.input_channels as usize;
        let stride = self.slot_stride()?;
        HEADER_BYTES
            .checked_add(
                stride
                    .checked_mul(SLOTS)
                    .ok_or(RegionError::BadShape { frames, channels })?,
            )
            .ok_or(RegionError::BadShape { frames, channels })
    }
}

impl Default for SessionLayout {
    fn default() -> Self {
        Self::stereo()
    }
}

#[repr(C)]
pub struct Header {
    pub magic: AtomicU64,
    pub published: AtomicU64,
    pub completed: AtomicU64,
    pub ready: AtomicU64,
    pub alarm: AtomicU64,
    pub notice: AtomicU64,
    pub dead: AtomicU64,
    pub epoch: AtomicU64,
    pub max_frames: AtomicU32,
    pub input_channels: AtomicU32,
    pub output_channels: AtomicU32,
    pub input_events: AtomicU32,
    pub output_events: AtomicU32,
    /// Why `dead` was set by the helper. 1: `process` returned an error.
    /// 2: a fatal signal (`SIGABRT`, `SIGSEGV`, `SIGBUS`, or `SIGILL`) ran the
    /// notice handler. 0: the process exited without that notice.
    pub fail_len: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<Header>() <= HEADER_BYTES);
const _: () = assert!(std::mem::align_of::<Header>() <= 8);

#[repr(C)]
struct SlotHead {
    command: AtomicU32,
    frames: AtomicU32,
    input_events: AtomicU32,
    output_events: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<SlotHead>() <= SLOT_HEAD);

#[derive(Debug)]
pub enum RegionError {
    Io(io::Error),
    Wait(WaitError),
    BadMagic,
    BadShape { frames: usize, channels: usize },
    UnknownCommand(u32),
    ProcessFailed(String),
    Lock(io::Error),
    Busy,
    Dead,
    TooManyEvents { count: usize, capacity: usize },
    Protocol(String),
}

impl std::fmt::Display for RegionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Wait(error) => write!(f, "{error}"),
            Self::BadMagic => f.write_str("shared mapping has the wrong magic"),
            Self::BadShape { frames, channels } => {
                write!(
                    f,
                    "block shape {channels}x{frames} does not fit the session"
                )
            }
            Self::UnknownCommand(command) => write!(f, "unknown command {command}"),
            Self::ProcessFailed(error) => write!(f, "plugin process failed: {error}"),
            Self::Lock(error) => write!(f, "mlock failed: {error}"),
            Self::Busy => f.write_str("both block slots are still in flight"),
            Self::Dead => f.write_str("plugin process has exited"),
            Self::TooManyEvents { count, capacity } => {
                write!(f, "{count} events exceed the session capacity {capacity}")
            }
            Self::Protocol(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RegionError {}

impl From<io::Error> for RegionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WaitError> for RegionError {
    fn from(error: WaitError) -> Self {
        Self::Wait(error)
    }
}

pub struct MappedRegion {
    map: MmapMut,
    file: File,
    layout: SessionLayout,
    stride: usize,
    wake_target: Option<std::sync::Arc<MappedRegion>>,
    /// Parent-only. A generation stored here is not read if it completes late.
    abandoned: AtomicU64,
}

/// One word every helper bumps when it finishes a block, so the audio thread
/// can wait once and still notice whichever helper finished.
pub struct FanIn {
    map: MmapMut,
    file: File,
}

impl std::fmt::Debug for FanIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FanIn")
    }
}

impl FanIn {
    pub fn create() -> Result<Self, RegionError> {
        let file = shm_file(4096)?;
        Self::from_file(file)
    }

    pub fn from_fd(fd: OwnedFd) -> Result<Self, RegionError> {
        Self::from_file(File::from(fd))
    }

    fn from_file(file: File) -> Result<Self, RegionError> {
        if file.metadata()?.len() < 8 {
            file.set_len(4096)?;
        }
        // SAFETY: the file is zero-filled and at least one atomic long.
        let map = unsafe { MmapMut::map_mut(&file)? };
        let fan = Self { map, file };
        fan.lock_pages()?;
        Ok(fan)
    }

    pub(crate) fn raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    pub fn load(&self) -> u64 {
        self.word().load(Ordering::Acquire)
    }

    pub fn bump(&self) {
        self.word().fetch_add(1, Ordering::Release);
        sync::wake_one(self.word(), SyncFlags::PROCESS);
    }

    /// Wait until the word changes or `deadline`. The remaining time is all
    /// that is left of the block, not a fresh timeout.
    pub fn wait_until(&self, seen: u64, deadline: Instant) -> Result<u64, RegionError> {
        if self.load() != seen {
            return Ok(self.load());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RegionError::Wait(WaitError::Timeout));
        }
        sync::wait_until_ne_timeout(self.word(), seen, remaining, SyncFlags::PROCESS)
            .map_err(Into::into)
    }

    fn word(&self) -> &AtomicU64 {
        // SAFETY: the mapping is alive, page-aligned, and at least 8 bytes.
        unsafe { &*self.map.as_ptr().cast::<AtomicU64>() }
    }

    fn lock_pages(&self) -> Result<(), RegionError> {
        // SAFETY: `map` is a live mapping.
        let rc = unsafe { libc::mlock(self.map.as_ptr().cast(), self.map.len()) };
        if rc != 0 {
            Err(RegionError::Lock(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

/// One decoded output event and the stage that produced it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delivered {
    pub event: Event,
    pub device: u32,
}

/// Result of looking at a submitted generation without waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collected {
    Pending,
    Ready { events: usize },
    Abandoned,
}

impl MappedRegion {
    /// File-backed mapping for in-process tests. Production uses [`Self::create_shared`].
    pub fn create(path: &Path) -> Result<Self, RegionError> {
        Self::create_with(path, SessionLayout::stereo())
    }

    pub fn create_with(path: &Path, layout: SessionLayout) -> Result<Self, RegionError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        map_new(file, layout)
    }

    /// `shm_open`, unlink the name, then lock the pages.
    pub fn create_shared(layout: SessionLayout) -> Result<Self, RegionError> {
        let len = layout.mapping_len()?;
        let file = shm_file(len)?;
        map_new(file, layout)
    }

    pub fn open(path: &Path) -> Result<Self, RegionError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        map_existing(file)
    }

    pub fn from_fd(fd: OwnedFd) -> Result<Self, RegionError> {
        map_existing(File::from(fd))
    }

    pub fn layout(&self) -> SessionLayout {
        self.layout
    }

    pub fn input_channels(&self) -> usize {
        self.layout.input_channels as usize
    }

    pub fn output_channels(&self) -> usize {
        self.layout.output_channels as usize
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    pub fn duplicate_fd(&self) -> io::Result<OwnedFd> {
        let fd = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: `fcntl` returned a new descriptor owned by this process.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    pub fn header(&self) -> &Header {
        // SAFETY: the mapping is page-aligned and at least one header long.
        unsafe { &*self.map.as_ptr().cast::<Header>() }
    }

    pub fn notice_ptr(&self) -> *const AtomicU64 {
        &raw const self.header().notice
    }

    pub fn dead_ptr(&self) -> *const AtomicU64 {
        &raw const self.header().dead
    }

    pub fn published(&self) -> u64 {
        self.header().published.load(Ordering::Acquire)
    }

    /// Increment the alarm without waking a waiter.
    /// The processing thread uses this when it is already running: waking
    /// itself is a syscall, and the next loop sees the new value.
    pub fn raise_alarm(&self) {
        self.header().alarm.fetch_add(1, Ordering::Release);
    }

    pub fn bump_alarm(&self) {
        self.raise_alarm();
        if let Some(target) = &self.wake_target {
            target.bump_alarm();
        } else {
            sync::wake_one(&self.header().alarm, SyncFlags::PROCESS);
        }
    }

    pub fn alarm(&self) -> u64 {
        self.header().alarm.load(Ordering::Acquire)
    }

    pub fn wait_alarm(&self, seen: u64) -> Result<u64, RegionError> {
        sync::wait_until_ne(&self.header().alarm, seen, SyncFlags::PROCESS).map_err(Into::into)
    }

    /// The plugin returned an error from `process`. The parent sees this on the
    /// next poll, without waiting for the block deadline.
    pub fn mark_process_refused(&self) {
        self.header().fail_len.store(1, Ordering::Release);
        self.header().dead.store(1, Ordering::Release);
    }

    /// A fatal signal ran in the helper. Written from that handler so the
    /// parent can tell a crash from a hang before the crash reporter exits.
    /// The store does not wake a thread already blocked in `os_sync_wait`.
    pub fn mark_aborted(&self) {
        self.header().fail_len.store(2, Ordering::Release);
        self.header().dead.store(1, Ordering::Release);
    }

    pub fn process_refused(&self) -> bool {
        self.header().fail_len.load(Ordering::Acquire) == 1
    }

    /// The fatal-signal handler stored its mark. Process exit alone does not.
    pub fn fatal_notice(&self) -> bool {
        self.header().fail_len.load(Ordering::Acquire) == 2
    }

    /// Keep the helper's wait address stable across prepared mapping changes.
    pub fn set_wake_target(&mut self, target: std::sync::Arc<MappedRegion>) {
        self.wake_target = Some(target);
    }

    pub fn is_ready(&self) -> bool {
        self.header().ready.load(Ordering::Acquire) != 0
    }

    pub fn is_dead(&self) -> bool {
        self.header().dead.load(Ordering::Acquire) != 0
            || self
                .wake_target
                .as_ref()
                .is_some_and(|target| target.is_dead())
    }

    /// Publish storage activation without processing or allocating audio.
    pub fn submit_activation(&self) -> Result<u64, RegionError> {
        if self.is_dead() {
            return Err(RegionError::Dead);
        }
        let generation = self.published().wrapping_add(1);
        if generation.wrapping_sub(self.header().completed.load(Ordering::Acquire)) > SLOTS as u64 {
            return Err(RegionError::Busy);
        }
        self.slot_head(generation)
            .command
            .store(crate::CMD_ADOPT_MAPPING, Ordering::Release);
        self.header().published.store(generation, Ordering::Release);
        self.bump_alarm();
        Ok(generation)
    }

    /// Control-thread activation for callers that are not publishing live audio.
    pub fn activate_mapping(&self, timeout: Duration) -> Result<(), RegionError> {
        let generation = self.submit_activation()?;
        self.wait_for_generation(generation, timeout)
    }

    pub fn mark_ready(&self) {
        self.header().ready.store(1, Ordering::Release);
        sync::wake_one(&self.header().ready, SyncFlags::PROCESS);
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<(), RegionError> {
        sync::wait_until_eq(&self.header().ready, 1, timeout, SyncFlags::PROCESS)?;
        Ok(())
    }

    /// Clear generations and slot headers before a new helper attaches.
    /// The caller is not inside `request`. The name stays unlinked; the fd is reused.
    pub fn reset_for_restart(&self) {
        let header = self.header();
        header.ready.store(0, Ordering::Release);
        header.dead.store(0, Ordering::Release);
        header.fail_len.store(0, Ordering::Release);
        header.published.store(0, Ordering::Release);
        header.completed.store(0, Ordering::Release);
        for index in 0..SLOTS {
            let head = self.slot_head_index(index);
            head.command.store(0, Ordering::Release);
            head.frames.store(0, Ordering::Release);
            head.input_events.store(0, Ordering::Release);
            head.output_events.store(0, Ordering::Release);
        }
        header.epoch.fetch_add(1, Ordering::Release);
        header.notice.fetch_add(1, Ordering::Release);
        sync::wake_one(&header.notice, SyncFlags::PROCESS);
        self.bump_alarm();
    }

    pub fn block_command(&self, generation: u64) -> u32 {
        self.slot_head(generation).command.load(Ordering::Acquire)
    }

    pub fn prepare_block(&self, generation: u64) -> Result<usize, RegionError> {
        let frames = self.slot_head(generation).frames.load(Ordering::Acquire) as usize;
        if frames == 0 || frames > self.layout.max_frames as usize {
            return Err(RegionError::BadShape {
                frames,
                channels: self.input_channels(),
            });
        }
        Ok(frames)
    }

    pub fn input_ptr(&self, generation: u64, channel: usize) -> *const f32 {
        self.plane(generation, false, channel)
    }

    pub fn output_ptr(&self, generation: u64, channel: usize) -> *mut f32 {
        self.plane(generation, true, channel).cast_mut()
    }

    pub fn copy_inputs_to_outputs(&self, generation: u64) -> Result<(), RegionError> {
        let frames = self.prepare_block(generation)?;
        let inputs = self.input_channels();
        let outputs = self.output_channels();
        for channel in 0..outputs {
            let destination = self.output_ptr(generation, channel);
            if channel < inputs {
                // SAFETY: both planes belong to this generation and `frames` fits.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.input_ptr(generation, channel),
                        destination,
                        frames,
                    );
                }
            } else {
                // SAFETY: the output plane is `frames` long.
                unsafe { std::ptr::write_bytes(destination, 0, frames) };
            }
        }
        Ok(())
    }

    pub fn silence_outputs(&self, generation: u64) -> Result<(), RegionError> {
        let frames = self.prepare_block(generation)?;
        for channel in 0..self.output_channels() {
            // SAFETY: the output plane is `frames` long.
            unsafe { std::ptr::write_bytes(self.output_ptr(generation, channel), 0, frames) };
        }
        Ok(())
    }

    /// Byte-copy input records into the output region. No typed read.
    pub fn echo_events(&self, generation: u64) {
        let count = self
            .slot_head(generation)
            .input_events
            .load(Ordering::Acquire)
            .min(self.layout.input_events)
            .min(self.layout.output_events);
        if count > 0 {
            let src = self.event_offset(generation, false, 0);
            let dst = self.event_offset(generation, true, 0);
            // SAFETY: the two event regions do not overlap and both fit in the slot.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(src),
                    self.map.as_ptr().cast_mut().add(dst),
                    count as usize * RECORD_BYTES,
                );
            }
        }
        self.slot_head(generation)
            .output_events
            .store(count, Ordering::Release);
    }

    /// Copy output planes back to input planes so the next stage reads them.
    pub fn feed_output_to_input(&self, generation: u64) -> Result<(), RegionError> {
        let frames = self.prepare_block(generation)?;
        let channels = self.input_channels().min(self.output_channels());
        for channel in 0..channels {
            // SAFETY: both planes belong to this generation and `frames` fits.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.output_ptr(generation, channel),
                    self.plane(generation, false, channel).cast_mut(),
                    frames,
                );
            }
        }
        Ok(())
    }

    /// Keep input records whose stage index is below `stages`. Others are dropped.
    /// The index is a plain `u32` in the record, not a Rust enum.
    pub fn echo_routed(&self, generation: u64, stages: u32) {
        let count = self
            .slot_head(generation)
            .input_events
            .load(Ordering::Acquire)
            .min(self.layout.input_events);
        let mut kept = 0u32;
        for index in 0..count {
            if kept >= self.layout.output_events {
                break;
            }
            let src = self.event_offset(generation, false, index as usize);
            let mut record = [0u8; RECORD_BYTES];
            // SAFETY: the record lies inside this generation's input events.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(src),
                    record.as_mut_ptr(),
                    RECORD_BYTES,
                );
            }
            let device = device_index(&record).unwrap_or(u32::MAX);
            if device >= stages {
                continue;
            }
            let dst = self.event_offset(generation, true, kept as usize);
            // SAFETY: the output record lies in the output event region, which
            // does not overlap the input region.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    self.map.as_ptr().cast_mut().add(dst),
                    RECORD_BYTES,
                );
            }
            kept += 1;
        }
        self.slot_head(generation)
            .output_events
            .store(kept, Ordering::Release);
    }

    /// Publish a block and return its generation. Does not wait.
    pub fn submit_block(
        &self,
        frames: usize,
        input: &[&[f32]],
        events: &[(Event, u32)],
    ) -> Result<u64, RegionError> {
        self.submit_block_with_reset(frames, input, events, false)
    }

    pub fn submit_block_with_reset(
        &self,
        frames: usize,
        input: &[&[f32]],
        events: &[(Event, u32)],
        reset: bool,
    ) -> Result<u64, RegionError> {
        if self.header().dead.load(Ordering::Acquire) != 0 {
            return Err(RegionError::Dead);
        }
        if frames == 0
            || frames > self.layout.max_frames as usize
            || input.len() != self.input_channels()
        {
            return Err(RegionError::BadShape {
                frames,
                channels: input.len(),
            });
        }
        if input.iter().any(|plane| plane.len() != frames) {
            return Err(RegionError::BadShape {
                frames,
                channels: input.len(),
            });
        }
        if events.len() > self.layout.input_events as usize {
            return Err(RegionError::TooManyEvents {
                count: events.len(),
                capacity: self.layout.input_events as usize,
            });
        }
        let header = self.header();
        let published = header.published.load(Ordering::Acquire);
        let completed = header.completed.load(Ordering::Acquire);
        if published.wrapping_sub(completed) >= SLOTS as u64 {
            return Err(RegionError::Busy);
        }
        let generation = published.wrapping_add(1);
        for (channel, plane) in input.iter().enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    plane.as_ptr(),
                    self.plane(generation, false, channel).cast_mut(),
                    frames,
                );
            }
        }
        for (index, (event, device)) in events.iter().enumerate() {
            let record = encode_routed(event, *device).ok_or_else(|| {
                RegionError::Protocol("event cannot be stored in a plain record".to_string())
            })?;
            let offset = self.event_offset(generation, false, index);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    self.map.as_ptr().cast_mut().add(offset),
                    RECORD_BYTES,
                );
            }
        }
        let head = self.slot_head(generation);
        head.input_events
            .store(events.len() as u32, Ordering::Release);
        head.output_events.store(0, Ordering::Release);
        head.frames.store(frames as u32, Ordering::Release);
        head.command.store(
            if reset {
                crate::CMD_RESET_PROCESS
            } else {
                CMD_PROCESS
            },
            Ordering::Release,
        );
        header.published.store(generation, Ordering::Release);
        self.bump_alarm();
        Ok(generation)
    }

    /// Publish a block from one buffer per channel. A buffer may be longer than
    /// `frames`; only the first `frames` samples are copied.
    pub fn submit_planes(
        &self,
        frames: usize,
        input: &[Vec<f32>],
        events: &[(Event, u32)],
    ) -> Result<u64, RegionError> {
        self.submit_planes_with_reset(frames, input, events, false)
    }

    pub fn submit_planes_with_reset(
        &self,
        frames: usize,
        input: &[Vec<f32>],
        events: &[(Event, u32)],
        reset: bool,
    ) -> Result<u64, RegionError> {
        if self.header().dead.load(Ordering::Acquire) != 0 {
            return Err(RegionError::Dead);
        }
        if frames == 0
            || frames > self.layout.max_frames as usize
            || input.len() != self.input_channels()
            || input.iter().any(|plane| plane.len() < frames)
        {
            return Err(RegionError::BadShape {
                frames,
                channels: input.len(),
            });
        }
        if events.len() > self.layout.input_events as usize {
            return Err(RegionError::TooManyEvents {
                count: events.len(),
                capacity: self.layout.input_events as usize,
            });
        }
        let header = self.header();
        let published = header.published.load(Ordering::Acquire);
        let completed = header.completed.load(Ordering::Acquire);
        if published.wrapping_sub(completed) >= SLOTS as u64 {
            return Err(RegionError::Busy);
        }
        let generation = published.wrapping_add(1);
        for (channel, plane) in input.iter().enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    plane.as_ptr(),
                    self.plane(generation, false, channel).cast_mut(),
                    frames,
                );
            }
        }
        for (index, (event, device)) in events.iter().enumerate() {
            let record = encode_routed(event, *device).ok_or_else(|| {
                RegionError::Protocol("event cannot be stored in a plain record".to_string())
            })?;
            let offset = self.event_offset(generation, false, index);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    self.map.as_ptr().cast_mut().add(offset),
                    RECORD_BYTES,
                );
            }
        }
        let head = self.slot_head(generation);
        head.input_events
            .store(events.len() as u32, Ordering::Release);
        head.output_events.store(0, Ordering::Release);
        head.frames.store(frames as u32, Ordering::Release);
        head.command.store(
            if reset {
                crate::CMD_RESET_PROCESS
            } else {
                CMD_PROCESS
            },
            Ordering::Release,
        );
        header.published.store(generation, Ordering::Release);
        self.bump_alarm();
        Ok(generation)
    }

    pub fn abandon(&self, generation: u64) {
        self.abandoned.store(generation, Ordering::Release);
    }

    /// Read a finished generation. Does not wait. A generation passed to
    /// [`Self::abandon`] is not read, even if the helper has since finished it.
    pub fn collect_ready(
        &self,
        generation: u64,
        output: &mut [&mut [f32]],
        delivered: &mut [Delivered],
    ) -> Result<Collected, RegionError> {
        if self.abandoned.load(Ordering::Acquire) == generation {
            return Ok(Collected::Abandoned);
        }
        if self.header().dead.load(Ordering::Acquire) != 0 {
            return Err(RegionError::Dead);
        }
        if self.header().completed.load(Ordering::Acquire) < generation {
            return Ok(Collected::Pending);
        }
        let frames = self.prepare_block(generation)?;
        if output.len() != self.output_channels() {
            return Err(RegionError::BadShape {
                frames,
                channels: output.len(),
            });
        }
        for plane in output.iter() {
            if plane.len() != frames {
                return Err(RegionError::BadShape {
                    frames,
                    channels: output.len(),
                });
            }
        }
        for (channel, plane) in output.iter_mut().enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.output_ptr(generation, channel),
                    plane.as_mut_ptr(),
                    frames,
                );
            }
        }
        let count = (self
            .slot_head(generation)
            .output_events
            .load(Ordering::Acquire) as usize)
            .min(self.layout.output_events as usize);
        let mut written = 0;
        for index in 0..count {
            if written == delivered.len() {
                break;
            }
            let offset = self.event_offset(generation, true, index);
            let mut record = [0u8; RECORD_BYTES];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(offset),
                    record.as_mut_ptr(),
                    RECORD_BYTES,
                );
            }
            if let Some(event) = decode_event(&record) {
                delivered[written] = Delivered {
                    event,
                    device: device_index(&record).unwrap_or(0),
                };
                written += 1;
            }
        }
        Ok(Collected::Ready { events: written })
    }

    /// Like [`Self::collect_ready`], writing `frames` samples into each buffer.
    pub fn collect_planes(
        &self,
        generation: u64,
        output: &mut [Vec<f32>],
        frames: usize,
        delivered: &mut [Delivered],
    ) -> Result<Collected, RegionError> {
        if self.abandoned.load(Ordering::Acquire) == generation {
            return Ok(Collected::Abandoned);
        }
        if self.header().dead.load(Ordering::Acquire) != 0 {
            return Err(RegionError::Dead);
        }
        if self.header().completed.load(Ordering::Acquire) < generation {
            return Ok(Collected::Pending);
        }
        let stored = self.prepare_block(generation)?;
        if stored != frames
            || output.len() != self.output_channels()
            || output.iter().any(|plane| plane.len() < frames)
        {
            return Err(RegionError::BadShape {
                frames,
                channels: output.len(),
            });
        }
        for (channel, plane) in output.iter_mut().enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.output_ptr(generation, channel),
                    plane.as_mut_ptr(),
                    frames,
                );
            }
        }
        let count = (self
            .slot_head(generation)
            .output_events
            .load(Ordering::Acquire) as usize)
            .min(self.layout.output_events as usize);
        let mut written = 0;
        for index in 0..count {
            if written == delivered.len() {
                break;
            }
            let offset = self.event_offset(generation, true, index);
            let mut record = [0u8; RECORD_BYTES];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(offset),
                    record.as_mut_ptr(),
                    RECORD_BYTES,
                );
            }
            if let Some(event) = decode_event(&record) {
                delivered[written] = Delivered {
                    event,
                    device: device_index(&record).unwrap_or(0),
                };
                written += 1;
            }
        }
        Ok(Collected::Ready { events: written })
    }

    /// Copies this generation's input events into `out`. `out` is cleared first
    /// and is not grown, so the caller reserves its capacity before the block.
    pub fn copy_input_events(&self, generation: u64, out: &mut Vec<Event>) {
        out.clear();
        let count = (self
            .slot_head(generation)
            .input_events
            .load(Ordering::Acquire) as usize)
            .min(self.layout.input_events as usize);
        for index in 0..count {
            if out.len() == out.capacity() {
                break;
            }
            let offset = self.event_offset(generation, false, index);
            let mut record = [0u8; RECORD_BYTES];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(offset),
                    record.as_mut_ptr(),
                    RECORD_BYTES,
                );
            }
            if let Some(event) = decode_event(&record) {
                out.push(event);
            }
        }
    }

    pub fn finish(&self, generation: u64) {
        let header = self.header();
        header.completed.store(generation, Ordering::Release);
        header.notice.fetch_add(1, Ordering::Release);
        sync::wake_one(&header.notice, SyncFlags::PROCESS);
    }

    pub fn request(
        &self,
        _seq: u64,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        command: u32,
        timeout: Duration,
    ) -> Result<(), RegionError> {
        let frames = infer_frames(self.layout, input, output)?;
        let mut none = [];
        self.request_events(frames, input, output, &[], &mut none, command, timeout)?;
        Ok(())
    }

    /// Publish one block. Returns how many valid output events were decoded.
    /// Invalid records are skipped. A timed-out generation is not read later.
    #[allow(clippy::too_many_arguments)] // planes, events, command, and the deadline are one block
    pub fn request_events(
        &self,
        frames: usize,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        events_in: &[Event],
        events_out: &mut [Event],
        command: u32,
        timeout: Duration,
    ) -> Result<usize, RegionError> {
        if self.header().dead.load(Ordering::Acquire) != 0 {
            return Err(RegionError::Dead);
        }
        check_planes(self.layout, input, output, frames)?;
        if events_in.len() > self.layout.input_events as usize {
            return Err(RegionError::TooManyEvents {
                count: events_in.len(),
                capacity: self.layout.input_events as usize,
            });
        }
        let header = self.header();
        let published = header.published.load(Ordering::Acquire);
        let completed = header.completed.load(Ordering::Acquire);
        if published.wrapping_sub(completed) >= SLOTS as u64 {
            return Err(RegionError::Busy);
        }
        let generation = published.wrapping_add(1);
        for (channel, plane) in input.iter().enumerate() {
            // SAFETY: the plane length is `frames` and the slot plane is at least that long.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    plane.as_ptr(),
                    self.plane(generation, false, channel).cast_mut(),
                    frames,
                );
            }
        }
        for (index, event) in events_in.iter().enumerate() {
            let record = encode_event(event).ok_or_else(|| {
                RegionError::Protocol("event cannot be stored in a plain record".to_string())
            })?;
            let offset = self.event_offset(generation, false, index);
            // SAFETY: `offset` addresses one record inside this generation's input events.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    self.map.as_ptr().cast_mut().add(offset),
                    RECORD_BYTES,
                );
            }
        }
        let head = self.slot_head(generation);
        head.input_events
            .store(events_in.len() as u32, Ordering::Release);
        head.output_events.store(0, Ordering::Release);
        head.frames.store(frames as u32, Ordering::Release);
        head.command.store(command, Ordering::Release);
        self.publish_and_wait(generation, timeout)?;
        if matches!(command, CMD_PROCESS | crate::CMD_RESET_PROCESS) {
            for (channel, plane) in output.iter_mut().enumerate() {
                // SAFETY: `finish` release-stored `completed` before we observed it.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.output_ptr(generation, channel),
                        plane.as_mut_ptr(),
                        frames,
                    );
                }
            }
        }
        Ok(self.read_output_events(generation, events_out))
    }

    fn publish_and_wait(&self, generation: u64, timeout: Duration) -> Result<(), RegionError> {
        let header = self.header();
        header.published.store(generation, Ordering::Release);
        self.bump_alarm();
        self.wait_for_generation(generation, timeout)
    }

    fn wait_for_generation(&self, generation: u64, timeout: Duration) -> Result<(), RegionError> {
        let header = self.header();
        let deadline = Instant::now() + timeout;
        let mut seen = header.notice.load(Ordering::Acquire);
        loop {
            if self.is_dead() {
                return Err(RegionError::Dead);
            }
            if header.completed.load(Ordering::Acquire) >= generation {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RegionError::Wait(WaitError::Timeout));
            }
            // Exit watches target the stable root. These blocking calls are
            // control-only; periodically inspect that root after a remap.
            let slice = if self.wake_target.is_some() {
                remaining.min(Duration::from_millis(20))
            } else {
                remaining
            };
            match sync::wait_until_ne_timeout(&header.notice, seen, slice, SyncFlags::PROCESS) {
                Ok(value) => seen = value,
                Err(WaitError::Timeout) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read_output_events(&self, generation: u64, events_out: &mut [Event]) -> usize {
        let count = (self
            .slot_head(generation)
            .output_events
            .load(Ordering::Acquire) as usize)
            .min(self.layout.output_events as usize);
        let mut written = 0;
        for index in 0..count {
            if written == events_out.len() {
                break;
            }
            let offset = self.event_offset(generation, true, index);
            let mut record = [0u8; RECORD_BYTES];
            // SAFETY: the record lies inside this generation's output event region.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.map.as_ptr().add(offset),
                    record.as_mut_ptr(),
                    RECORD_BYTES,
                );
            }
            if let Some(event) = decode_event(&record) {
                events_out[written] = event;
                written += 1;
            }
        }
        written
    }

    fn init_header(&self) {
        let header = self.header();
        header.published.store(0, Ordering::Relaxed);
        header.completed.store(0, Ordering::Relaxed);
        header.ready.store(0, Ordering::Relaxed);
        header.alarm.store(0, Ordering::Relaxed);
        header.notice.store(0, Ordering::Relaxed);
        header.dead.store(0, Ordering::Relaxed);
        header.epoch.store(0, Ordering::Relaxed);
        header
            .max_frames
            .store(self.layout.max_frames, Ordering::Relaxed);
        header
            .input_channels
            .store(self.layout.input_channels, Ordering::Relaxed);
        header
            .output_channels
            .store(self.layout.output_channels, Ordering::Relaxed);
        header
            .input_events
            .store(self.layout.input_events, Ordering::Relaxed);
        header
            .output_events
            .store(self.layout.output_events, Ordering::Relaxed);
        header.magic.store(MAGIC, Ordering::Release);
    }

    fn lock_pages(&self) -> Result<(), RegionError> {
        // SAFETY: `map` is a live mapping. `mlock` pins those pages.
        let rc = unsafe { libc::mlock(self.map.as_ptr().cast(), self.map.len()) };
        if rc != 0 {
            Err(RegionError::Lock(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    fn slot_offset(&self, generation: u64) -> usize {
        debug_assert!(generation >= 1);
        let index = ((generation - 1) % SLOTS as u64) as usize;
        HEADER_BYTES + index * self.stride
    }

    fn slot_head(&self, generation: u64) -> &SlotHead {
        self.slot_head_index((generation - 1) as usize % SLOTS)
    }

    fn slot_head_index(&self, index: usize) -> &SlotHead {
        let offset = HEADER_BYTES + index * self.stride;
        // SAFETY: each slot starts inside the mapping, aligned for `AtomicU32`.
        unsafe { &*self.map.as_ptr().add(offset).cast::<SlotHead>() }
    }

    fn plane(&self, generation: u64, output: bool, channel: usize) -> *const f32 {
        let before = if output { self.input_channels() } else { 0 };
        let offset = self.slot_offset(generation)
            + SLOT_HEAD
            + (before + channel) * self.layout.max_frames as usize * std::mem::size_of::<f32>();
        debug_assert!(offset + std::mem::size_of::<f32>() <= self.map.len());
        // SAFETY: `offset` is inside this generation's audio planes.
        unsafe { self.map.as_ptr().add(offset).cast::<f32>() }
    }

    fn event_offset(&self, generation: u64, output: bool, index: usize) -> usize {
        let audio = (self.input_channels() + self.output_channels())
            * self.layout.max_frames as usize
            * std::mem::size_of::<f32>();
        let skip = if output {
            self.layout.input_events as usize
        } else {
            0
        };
        self.slot_offset(generation) + SLOT_HEAD + audio + (skip + index) * RECORD_BYTES
    }
}

fn map_new(file: File, layout: SessionLayout) -> Result<MappedRegion, RegionError> {
    layout.validate()?;
    let stride = layout.slot_stride()?;
    let total = layout.mapping_len()? as u64;
    // `File::set_len` rejects a shared-memory object on macOS. `shm_file`
    // already sized it with `ftruncate`. Regular files still need `set_len`.
    if file.metadata()?.len() < total {
        file.set_len(total)?;
    }
    // SAFETY: the file is zero-filled and long enough for the header.
    let map = unsafe { MmapMut::map_mut(&file) }?;
    let region = MappedRegion {
        map,
        file,
        layout,
        stride,
        abandoned: AtomicU64::new(0),
        wake_target: None,
    };
    region.init_header();
    region.lock_pages()?;
    Ok(region)
}

fn map_existing(file: File) -> Result<MappedRegion, RegionError> {
    // SAFETY: the creator initialized this file before handing over the descriptor.
    let map = unsafe { MmapMut::map_mut(&file)? };
    if map.len() < HEADER_BYTES {
        return Err(RegionError::BadMagic);
    }
    let header = unsafe { &*map.as_ptr().cast::<Header>() };
    if header.magic.load(Ordering::Acquire) != MAGIC {
        return Err(RegionError::BadMagic);
    }
    let layout = SessionLayout {
        max_frames: header.max_frames.load(Ordering::Acquire),
        input_channels: header.input_channels.load(Ordering::Acquire),
        output_channels: header.output_channels.load(Ordering::Acquire),
        input_events: header.input_events.load(Ordering::Acquire),
        output_events: header.output_events.load(Ordering::Acquire),
    };
    layout.validate()?;
    let stride = layout.slot_stride()?;
    if map.len() < layout.mapping_len()? {
        return Err(RegionError::BadMagic);
    }
    let region = MappedRegion {
        map,
        file,
        layout,
        stride,
        abandoned: AtomicU64::new(0),
        wake_target: None,
    };
    region.lock_pages()?;
    Ok(region)
}

fn shm_file(len: usize) -> Result<File, RegionError> {
    let name = format!(
        "/echode-{}-{}",
        std::process::id(),
        SHM_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let name = CString::new(name)
        .map_err(|error| RegionError::Io(io::Error::new(io::ErrorKind::InvalidInput, error)))?;
    // `mode_t` is `u16` on macOS, and a variadic argument is passed as `int`.
    let fd = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600 as libc::c_int,
        )
    };
    if fd < 0 {
        return Err(io::Error::other(format!("shm_open: {}", io::Error::last_os_error())).into());
    }
    if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
        let error = io::Error::other(format!("shm_unlink: {}", io::Error::last_os_error()));
        unsafe { libc::close(fd) };
        return Err(error.into());
    }
    if unsafe { libc::ftruncate(fd, len as i64) } != 0 {
        let error = io::Error::other(format!("ftruncate: {}", io::Error::last_os_error()));
        unsafe { libc::close(fd) };
        return Err(error.into());
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
    // SAFETY: `fd` is the shared-memory object this function opened.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn infer_frames(
    layout: SessionLayout,
    input: &[&[f32]],
    output: &[&mut [f32]],
) -> Result<usize, RegionError> {
    let frames = if layout.input_channels > 0 {
        input.first().map(|plane| plane.len()).unwrap_or(0)
    } else if layout.output_channels > 0 {
        output.first().map(|plane| plane.len()).unwrap_or(0)
    } else {
        return Err(RegionError::BadShape {
            frames: 0,
            channels: 0,
        });
    };
    check_planes(layout, input, output, frames)?;
    Ok(frames)
}

fn check_planes(
    layout: SessionLayout,
    input: &[&[f32]],
    output: &[&mut [f32]],
    frames: usize,
) -> Result<(), RegionError> {
    if input.len() != layout.input_channels as usize
        || output.len() != layout.output_channels as usize
        || frames == 0
        || frames > layout.max_frames as usize
        || input.iter().any(|plane| plane.len() != frames)
        || output.iter().any(|plane| plane.len() != frames)
    {
        return Err(RegionError::BadShape {
            frames,
            channels: input.len(),
        });
    }
    Ok(())
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    let rounded = value.checked_add(align - 1)?;
    Some(rounded & !(align - 1))
}

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;
