//! The shared mapping. One [`Header`] plus planar f32 audio.

use crate::sync::{self, SyncFlags, WaitError};
use crate::{CMD_PROCESS, CMD_SHUTDOWN, MAGIC, MAX_CHANNELS, MAX_FRAMES};
use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

#[repr(C)]
pub struct Header {
    pub magic: AtomicU64,
    pub request: AtomicU64,
    pub response: AtomicU64,
    pub ready: AtomicU64,
    pub frames: AtomicU32,
    pub channels: AtomicU32,
    pub command: AtomicU32,
    pub _pad: u32,
}

#[repr(C)]
pub struct Region {
    pub header: Header,
    pub input: [[f32; MAX_FRAMES]; MAX_CHANNELS],
    pub output: [[f32; MAX_FRAMES]; MAX_CHANNELS],
}

const _: () = assert!(std::mem::align_of::<Header>() >= 8);
const _: () = assert!(std::mem::size_of::<Region>() < 64 * 1024);

#[derive(Debug)]
pub enum RegionError {
    Io(std::io::Error),
    Wait(WaitError),
    BadMagic,
    BadShape { frames: usize, channels: usize },
    UnknownCommand(u32),
    ProcessFailed(String),
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
                    "block shape {channels}x{frames} is outside the spike mapping"
                )
            }
            Self::UnknownCommand(command) => write!(f, "unknown command {command}"),
            Self::ProcessFailed(error) => write!(f, "plugin process failed: {error}"),
        }
    }
}

impl std::error::Error for RegionError {}

impl From<std::io::Error> for RegionError {
    fn from(error: std::io::Error) -> Self {
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
}

impl MappedRegion {
    pub fn create(path: &Path) -> Result<Self, RegionError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        file.set_len(std::mem::size_of::<Region>() as u64)?;
        // SAFETY: the file is zero-filled and at least one region long. The
        // mapping is page-aligned, which satisfies Header's alignment.
        let map = unsafe { MmapMut::map_mut(&file)? };
        let region = Self { map };
        region.header().magic.store(MAGIC, Ordering::Release);
        Ok(region)
    }

    pub fn open(path: &Path) -> Result<Self, RegionError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: the parent created this file with `create` before spawning us.
        let map = unsafe { MmapMut::map_mut(&file)? };
        let region = Self { map };
        if region.header().magic.load(Ordering::Acquire) != MAGIC {
            return Err(RegionError::BadMagic);
        }
        Ok(region)
    }

    pub fn header(&self) -> &Header {
        // SAFETY: the mapping is alive, aligned, and initialized to a Header.
        unsafe { &*self.map.as_ptr().cast::<Header>() }
    }

    fn plane(&self, output: bool, channel: usize) -> *mut f32 {
        debug_assert!(channel < MAX_CHANNELS);
        let audio_offset = if output {
            std::mem::offset_of!(Region, output)
        } else {
            std::mem::offset_of!(Region, input)
        };
        let byte_offset = audio_offset + channel * MAX_FRAMES * std::mem::size_of::<f32>();
        // SAFETY: `byte_offset` lands inside the mapped Region, on an f32.
        unsafe { self.map.as_ptr().add(byte_offset).cast::<f32>().cast_mut() }
    }

    pub fn mark_ready(&self) {
        self.header().ready.store(1, Ordering::Release);
        sync::wake_one(&self.header().ready, SyncFlags::PROCESS);
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<(), RegionError> {
        sync::wait_until_eq(&self.header().ready, 1, timeout, SyncFlags::PROCESS)?;
        Ok(())
    }

    /// Publish one stereo block and wait until the other process has answered.
    pub fn request(
        &self,
        seq: u64,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        command: u32,
        timeout: Duration,
    ) -> Result<(), RegionError> {
        let frames = check_shape(input, output)?;
        for (channel, plane) in input.iter().enumerate() {
            // SAFETY: `plane` has `frames` samples and the destination plane is `MAX_FRAMES` long.
            unsafe {
                std::ptr::copy_nonoverlapping(plane.as_ptr(), self.plane(false, channel), frames);
            }
        }
        let header = self.header();
        header.frames.store(frames as u32, Ordering::Relaxed);
        header.channels.store(input.len() as u32, Ordering::Relaxed);
        header.command.store(command, Ordering::Relaxed);
        header.request.store(seq, Ordering::Release);
        sync::wake_one(&header.request, SyncFlags::PROCESS);
        sync::wait_until_eq(&header.response, seq, timeout, SyncFlags::PROCESS)?;
        if command == CMD_PROCESS {
            for (channel, plane) in output.iter_mut().enumerate() {
                // SAFETY: the child release-stored `response` before we observed it,
                // so its writes to this plane are visible. Lengths were checked above.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.plane(true, channel),
                        plane.as_mut_ptr(),
                        frames,
                    );
                }
            }
        }
        Ok(())
    }

    /// Child-side loop. `process` reads input planes and writes output planes.
    pub fn serve(
        &self,
        mut process: impl FnMut(&MappedRegion, usize, usize) -> Result<(), RegionError>,
    ) -> Result<(), RegionError> {
        self.mark_ready();
        let mut seen = 0u64;
        loop {
            let seq = sync::wait_until_ne(&self.header().request, seen, SyncFlags::PROCESS)?;
            seen = seq;
            let header = self.header();
            let command = header.command.load(Ordering::Acquire);
            if command == CMD_SHUTDOWN {
                header.response.store(seq, Ordering::Release);
                sync::wake_one(&header.response, SyncFlags::PROCESS);
                return Ok(());
            }
            if command != CMD_PROCESS {
                return Err(RegionError::UnknownCommand(command));
            }
            let frames = header.frames.load(Ordering::Acquire) as usize;
            let channels = header.channels.load(Ordering::Acquire) as usize;
            if channels > MAX_CHANNELS || frames > MAX_FRAMES {
                return Err(RegionError::BadShape { frames, channels });
            }
            process(self, frames, channels)?;
            header.response.store(seq, Ordering::Release);
            sync::wake_one(&header.response, SyncFlags::PROCESS);
        }
    }

    pub fn input_ptr(&self, channel: usize) -> *const f32 {
        self.plane(false, channel)
    }

    pub fn output_ptr(&self, channel: usize) -> *mut f32 {
        self.plane(true, channel)
    }
}

fn check_shape(input: &[&[f32]], output: &[&mut [f32]]) -> Result<usize, RegionError> {
    if input.len() != MAX_CHANNELS || output.len() != MAX_CHANNELS {
        return Err(RegionError::BadShape {
            frames: input.first().map_or(0, |plane| plane.len()),
            channels: input.len(),
        });
    }
    let frames = input[0].len();
    let fits = frames > 0
        && frames <= MAX_FRAMES
        && input.iter().all(|plane| plane.len() == frames)
        && output.iter().all(|plane| plane.len() == frames);
    if !fits {
        return Err(RegionError::BadShape {
            frames,
            channels: input.len(),
        });
    }
    Ok(frames)
}
