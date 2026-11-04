//! Offline sample-peak normalization, after mixing and before dither/quantization.
//! Spool once rather than running stateful plugins a second time.
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::{Cancellation, CliError, Dither, WavFile, WavFormat, temporary_wav_path};

#[derive(Clone, Copy)]
pub(crate) struct PeakTarget(f64);

impl PeakTarget {
    pub(crate) fn parse(text: &str) -> Result<Self, CliError> {
        let db: f64 = text.parse().map_err(|_| Self::invalid())?;
        if !db.is_finite() || !(-120.0..=0.0).contains(&db) {
            return Err(Self::invalid());
        }
        Ok(Self(10.0f64.powf(db / 20.0)))
    }

    fn invalid() -> CliError {
        CliError::new("peak target must be a finite dBFS value from -120 to 0")
    }
}

pub(crate) struct ExportOutput {
    wav: WavFile,
    normalization: Option<Normalization>,
}

struct Normalization {
    spool: BufWriter<File>,
    bytes: Vec<u8>,
    samples: u64,
    peak: f64,
    target: PeakTarget,
}

impl ExportOutput {
    pub(crate) fn create(
        path: &Path,
        rate: u32,
        frames: u32,
        format: WavFormat,
        dither: Dither,
        target: Option<PeakTarget>,
    ) -> Result<Self, CliError> {
        let wav = WavFile::create_with_dither(path, rate, frames, format, dither)?;
        let normalization = target
            .map(|target| Normalization::create(path, target))
            .transpose()?;
        Ok(Self { wav, normalization })
    }

    pub(crate) fn write_block(&mut self, samples: &[f32]) -> Result<(), CliError> {
        if let Some(normalization) = &mut self.normalization {
            normalization.write_block(samples)
        } else {
            self.wav.write_block(samples)
        }
    }

    pub(crate) fn finish_cancellable(
        mut self,
        cancellation: &Cancellation,
    ) -> Result<(), CliError> {
        if let Some(normalization) = &mut self.normalization {
            normalization.encode(&mut self.wav, cancellation)?;
        }
        self.wav.finish_cancellable(cancellation)
    }
}

impl Normalization {
    fn create(path: &Path, target: PeakTarget) -> Result<Self, CliError> {
        let temporary = temporary_wav_path(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(scratch_error)?;
        // The descriptor owns the scratch lifetime, even on cancellation or crash.
        std::fs::remove_file(&temporary).map_err(scratch_error)?;
        Ok(Self {
            spool: BufWriter::with_capacity(64 * 1024, file),
            bytes: Vec::new(),
            samples: 0,
            peak: 0.0,
            target,
        })
    }

    fn write_block(&mut self, samples: &[f32]) -> Result<(), CliError> {
        self.bytes.clear();
        for &sample in samples {
            if !sample.is_finite() {
                return Err(CliError::new("non-finite sample in peak-normalized export"));
            }
            self.peak = self.peak.max(f64::from(sample).abs());
            self.bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.spool.write_all(&self.bytes).map_err(scratch_error)?;
        self.samples += samples.len() as u64;
        Ok(())
    }

    fn encode(&mut self, wav: &mut WavFile, cancellation: &Cancellation) -> Result<(), CliError> {
        cancellation.check()?;
        self.spool.flush().map_err(scratch_error)?;
        self.spool.seek(SeekFrom::Start(0)).map_err(scratch_error)?;
        // f64 also handles finite f32 subnormals without an overflowing gain.
        let gain = if self.peak == 0.0 {
            1.0
        } else {
            self.target.0 / self.peak
        };
        // Fixed scratch per encoding block, not a limit on song length or channels.
        let mut bytes = [0u8; 8192];
        let mut samples = [0f32; 2048];
        let mut remaining = self.samples;
        while remaining > 0 {
            cancellation.check()?;
            let count = remaining.min(samples.len() as u64) as usize;
            self.spool
                .get_mut()
                .read_exact(&mut bytes[..count * 4])
                .map_err(scratch_error)?;
            for (sample, raw) in samples[..count].iter_mut().zip(bytes.chunks_exact(4)) {
                let value = f32::from_le_bytes(raw.try_into().expect("four-byte sample"));
                *sample = (f64::from(value) * gain) as f32;
            }
            wav.write_block(&samples[..count])?;
            remaining -= count as u64;
        }
        Ok(())
    }
}

fn scratch_error(error: std::io::Error) -> CliError {
    CliError::new(format!("normalization scratch I/O failed: {error}"))
}

#[cfg(test)]
#[path = "normalization_tests.rs"]
mod tests;
