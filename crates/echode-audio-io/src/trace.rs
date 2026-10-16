//! Opt-in recording of buffers actually submitted to HAL, without another
//! stream or renderer. The callback owns one preallocated rolling buffer and
//! transfers it to the control thread after a stop. Only control writes files.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use echode_engine::PlaybackStatus;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use serde::Serialize;

#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Block {
    pub first_frame: u64,
    pub frames: usize,
    pub sample: i64,
    pub entry_ns: u64,
    pub elapsed_ns: u64,
    pub render_ns: u64,
    pub output_host_time: u64,
    pub transport_revision: u64,
    pub output_revision: u64,
    pub authorized: bool,
    pub stop_fade_offset: Option<u32>,
    pub hardware_changed: bool,
    pub invalid_buffer: bool,
    pub callback_failed: bool,
    pub engine_failed: bool,
    pub plugin_missed_blocks: u64,
    pub cleanup_pending: bool,
}

struct Buffer {
    pcm: Vec<[f32; 2]>,
    blocks: Vec<Block>,
    total: u64,
    block_count: u64,
    trigger: Option<u64>,
}

pub(crate) struct Writer {
    buffer: Option<Box<Buffer>>,
    ready: Producer<Box<Buffer>>,
    recycled: Consumer<Box<Buffer>>,
    origin: Instant,
    post_frames: u64,
}

pub(crate) struct Files {
    directory: PathBuf,
    rate: u32,
    ready: Consumer<Box<Buffer>>,
    recycled: Producer<Box<Buffer>>,
    disabled: bool,
    sequence: u64,
}

pub(crate) fn configured(rate: u32, frames: usize) -> Option<(Writer, Files)> {
    let directory = std::env::var_os("ECHODE_STOP_TRACE_DIR")
        .or_else(|| option_env!("ECHODE_STOP_TRACE_DIR").map(std::ffi::OsString::from))?;
    if directory.is_empty() {
        return None;
    }
    let directory = PathBuf::from(directory);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!("Could not enable stop diagnostics: {error}");
        return None;
    }
    Some(pair(rate, frames, directory))
}

pub(crate) fn pair(rate: u32, frames: usize, directory: PathBuf) -> (Writer, Files) {
    // Half a second plus two device blocks bounds this diagnostic recording,
    // not product capacity. Only the first two submitted output channels are
    // recorded; a mono device is duplicated for inspection. Metadata capacity
    // also accommodates one-frame callbacks without allocating on RT.
    let capacity = (rate as usize / 2)
        .saturating_add(frames.saturating_mul(2))
        .max(1);
    let buffer = Box::new(Buffer {
        pcm: vec![[0.0; 2]; capacity],
        blocks: vec![Block::default(); capacity + 1],
        total: 0,
        block_count: 0,
        trigger: None,
    });
    let (ready_tx, ready_rx) = RingBuffer::new(1);
    let (recycled_tx, recycled_rx) = RingBuffer::new(1);
    (
        Writer {
            buffer: Some(buffer),
            ready: ready_tx,
            recycled: recycled_rx,
            origin: Instant::now(),
            post_frames: u64::from((rate / 20).max(1)),
        },
        Files {
            directory,
            rate,
            ready: ready_rx,
            recycled: recycled_tx,
            disabled: false,
            sequence: 0,
        },
    )
}

impl Writer {
    #[cfg_attr(feature = "rtsan", rtsan_standalone::nonblocking)]
    pub(crate) fn record(
        &mut self,
        mut block: Block,
        entered: Instant,
        stop: bool,
        mut sample: impl FnMut(usize) -> [f32; 2],
    ) {
        if self.buffer.is_none() {
            self.buffer = self.recycled.pop().ok();
        }
        let Some(buffer) = &mut self.buffer else {
            return;
        };
        block.first_frame = buffer.total;
        block.entry_ns = entered
            .saturating_duration_since(self.origin)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        if stop && buffer.trigger.is_none() {
            buffer.trigger = Some(buffer.total);
        }
        for frame in 0..block.frames {
            let slot = (buffer.total % buffer.pcm.len() as u64) as usize;
            buffer.pcm[slot] = sample(frame);
            buffer.total += 1;
        }
        block.elapsed_ns = entered.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let slot = (buffer.block_count % buffer.blocks.len() as u64) as usize;
        buffer.blocks[slot] = block;
        buffer.block_count += 1;
        if buffer.trigger.is_some_and(|trigger| {
            buffer.total.saturating_sub(trigger) >= self.post_frames || block.invalid_buffer
        }) {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if let Some(buffer) = self.buffer.take()
            && let Err(PushError::Full(buffer)) = self.ready.push(buffer)
        {
            // Never drop a recording's storage on the audio thread.
            self.buffer = Some(buffer);
        }
    }
}

impl Files {
    pub(crate) fn service(&mut self, status: PlaybackStatus) {
        let Ok(mut buffer) = self.ready.pop() else {
            return;
        };
        if !self.disabled
            && let Err(error) = self.save(&buffer, status)
        {
            self.disabled = true;
            eprintln!("Stop diagnostics disabled after a write failure: {error}");
        }
        buffer.total = 0;
        buffer.block_count = 0;
        buffer.trigger = None;
        // A single owned buffer traverses these two queues. Control has popped
        // it from ready, so recycled cannot already hold another buffer.
        let _ = self.recycled.push(buffer);
    }

    fn save(&mut self, buffer: &Buffer, status: PlaybackStatus) -> std::io::Result<()> {
        let first = buffer.total.saturating_sub(buffer.pcm.len() as u64);
        let frames = buffer.total - first;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stem = format!("stop-{timestamp}-{}-{}", std::process::id(), self.sequence);
        self.sequence += 1;
        let pcm_name = format!("{stem}.f32le");
        let mut pcm = BufWriter::new(std::fs::File::create(self.directory.join(&pcm_name))?);
        for index in first..buffer.total {
            let sample = buffer.pcm[(index % buffer.pcm.len() as u64) as usize];
            for value in sample {
                pcm.write_all(&value.to_le_bytes())?;
            }
        }
        pcm.flush()?;
        let first_block = buffer
            .block_count
            .saturating_sub(buffer.blocks.len() as u64);
        let blocks: Vec<_> = (first_block..buffer.block_count)
            .map(|index| buffer.blocks[(index % buffer.blocks.len() as u64) as usize])
            .filter(|block| {
                block.first_frame.saturating_add(block.frames as u64) > first
                    || block.first_frame >= first
            })
            .collect();
        let report = serde_json::json!({
            "version": 1, "format": "stereo-f32-le", "sample_rate": self.rate,
            "build_source": option_env!("ECHODE_TRACE_BUILD"),
            "pcm": pcm_name, "frames": frames, "first_frame": first,
            "stop_frame": buffer.trigger,
            "stop_retained": buffer.trigger.is_some_and(|trigger| trigger >= first),
            "blocks": blocks,
            "status_after_capture": {
                "playing": status.playing, "failed": status.failed,
                "plugin_missed_blocks": status.plugin_missed_blocks,
                "graph_unavailable_blocks": status.graph_unavailable_blocks,
                "pending_plan": status.pending_plan,
            },
        });
        let path = self.directory.join(format!("{stem}.json"));
        let temporary = path.with_extension("json.part");
        let mut metadata = BufWriter::new(std::fs::File::create(&temporary)?);
        serde_json::to_writer(&mut metadata, &report)?;
        metadata.flush()?;
        std::fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_trace_transfers_storage_and_preserves_nonfinite_sample_bits_under_rtsan() {
        let directory =
            std::env::temp_dir().join(format!("echode-stop-ring-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (mut writer, mut files) = pair(1000, 7, directory.clone());
        for index in 0..110 {
            let guard = echode_engine::ForbidAlloc::enter();
            writer.record(
                Block {
                    frames: 7,
                    ..Default::default()
                },
                Instant::now(),
                index == 100,
                |frame| [f32::from_bits(0x7fc0_0013), (index * 7 + frame) as f32],
            );
            drop(guard);
        }
        files.service(PlaybackStatus::default());
        let path = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(metadata["stop_frame"], 700);
        assert_eq!(metadata["first_frame"], 242);
        assert_eq!(metadata["frames"], 514);
        let pcm = std::fs::read(directory.join(metadata["pcm"].as_str().unwrap())).unwrap();
        for (frame, bytes) in pcm.chunks_exact(8).enumerate() {
            assert_eq!(
                u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                0x7fc0_0013
            );
            assert_eq!(
                f32::from_le_bytes(bytes[4..].try_into().unwrap()),
                (frame + 242) as f32
            );
        }
        // The callback receives the same storage back, rather than allocating
        // another ring after each capture.
        let guard = echode_engine::ForbidAlloc::enter();
        writer.record(
            Block {
                frames: 7,
                ..Default::default()
            },
            Instant::now(),
            false,
            |_| [0.0; 2],
        );
        drop(guard);
        assert!(writer.buffer.is_some());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
