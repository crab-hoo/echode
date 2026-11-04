//! File-boundary dither is independent of the engine's callback partition.

use super::dither::Dither;
use super::wav_tests::WRITER;
use super::{WavFile, WavFormat};

fn scratch() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "echode-tpdf-writer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn existing_triangular_sampler_has_unit_lsb_support_and_uncorrelated_channels() {
    let mut generator = Dither::Tpdf.generator().unwrap();
    let (mut sum, mut power, mut stereo, mut lag) = (0.0, 0.0, 0.0, 0.0);
    let mut previous = 0.0;
    // This population checks a fixed seed, not a statistical runtime gate.
    let frames = 131_072;
    for _ in 0..frames {
        let left = generator.sample_lsb();
        let right = generator.sample_lsb();
        assert!((-1.0..=1.0).contains(&left));
        assert!((-1.0..=1.0).contains(&right));
        sum += left + right;
        power += left * left + right * right;
        stereo += left * right;
        lag += previous * left + left * right;
        previous = right;
    }
    let count = f64::from(frames * 2);
    assert!((sum / count).abs() < 0.005);
    assert!((power / count - 1.0 / 6.0).abs() < 0.005);
    assert!((stereo / f64::from(frames)).abs() < 0.005);
    assert!((lag / count).abs() < 0.005);
}

#[test]
fn dithered_pcm_is_identical_across_writer_partitions_and_saturates_without_wrapping() {
    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let wav = directory.join("out.wav");
    for (format, bits, header) in [(WavFormat::Pcm16, 16, 44), (WavFormat::Pcm24, 24, 68)] {
        let scale = (1u32 << (bits - 1)) as f32;
        // These samples and partitions exercise only this writer regression.
        let samples: Vec<_> = (0..1027)
            .flat_map(|_| {
                [
                    -f32::MAX,
                    f32::MAX,
                    -2.0,
                    2.0,
                    -1.0,
                    1.0,
                    -0.5,
                    0.25,
                    -0.25 / scale,
                    0.75 / scale,
                    0.0,
                    -0.0,
                ]
            })
            .collect();
        let frames = u32::try_from(samples.len() / 2).unwrap();
        let mut reference = None;
        for partition in [1, 2, 14, 128, 512, 4096] {
            let mut writer =
                WavFile::create_with_dither(&wav, 48_000, frames, format, Dither::Tpdf).unwrap();
            // Empty writes must not consume any noise, even between real blocks.
            for block in samples.chunks(partition) {
                writer.write_block(&[]).unwrap();
                writer.write_block(block).unwrap();
            }
            writer.finish().unwrap();
            let bytes = std::fs::read(&wav).unwrap();
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference, "{bits} bit partition {partition}");
            } else {
                reference = Some(bytes.clone());
            }
            for (index, data) in bytes[header..].chunks_exact(bits / 8).enumerate() {
                let integer = if bits == 16 {
                    i32::from(i16::from_le_bytes(data.try_into().unwrap()))
                } else {
                    i32::from_le_bytes([0, data[0], data[1], data[2]]) >> 8
                };
                let target = (f64::from(samples[index]) * f64::from(scale))
                    .clamp(-f64::from(scale), f64::from(scale) - 1.0);
                assert!((f64::from(integer) - target).abs() <= 1.5);
                if index % 12 < 4 {
                    assert_eq!(f64::from(integer), target, "over-range must saturate");
                }
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tpdf_quantization_error_keeps_zero_mean_and_quarter_lsb_power_at_fractional_inputs() {
    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let wav = directory.join("out.wav");
    for (format, bits, header) in [(WavFormat::Pcm16, 16, 44), (WavFormat::Pcm24, 24, 68)] {
        for fraction in [-0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75] {
            let scale = (1u32 << (bits - 1)) as f32;
            // A fixed population checks signal-independent error at sub-LSB offsets.
            let samples = vec![fraction / scale; 65_536];
            let mut writer =
                WavFile::create_with_dither(&wav, 48_000, 32_768, format, Dither::Tpdf).unwrap();
            writer.write_block(&samples).unwrap();
            writer.finish().unwrap();
            let bytes = std::fs::read(&wav).unwrap();
            let (mut sum, mut power) = (0.0, 0.0);
            for data in bytes[header..].chunks_exact(bits / 8) {
                let integer = if bits == 16 {
                    i32::from(i16::from_le_bytes(data.try_into().unwrap()))
                } else {
                    i32::from_le_bytes([0, data[0], data[1], data[2]]) >> 8
                };
                let error = f64::from(integer) - f64::from(fraction);
                assert!(error.abs() <= 1.5);
                sum += error;
                power += error * error;
            }
            assert!((sum / samples.len() as f64).abs() < 0.01);
            assert!((power / samples.len() as f64 - 0.25).abs() < 0.01);
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn dither_preserves_transactional_errors_and_rejects_nonfinite_pcm() {
    use super::{
        WAV_FAULT_DIR_SYNC, WAV_FAULT_FLUSH, WAV_FAULT_RENAME, WAV_FAULT_WRITE, arm_wav_fault,
    };

    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let wav = directory.join("keep.wav");
    for format in [WavFormat::Pcm16, WavFormat::Pcm24] {
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            std::fs::write(&wav, b"keep").unwrap();
            let mut writer =
                WavFile::create_with_dither(&wav, 48_000, 2, format, Dither::Tpdf).unwrap();
            writer.write_block(&[0.0, 0.0]).unwrap();
            assert!(
                writer
                    .write_block(&[0.0, sample])
                    .unwrap_err()
                    .to_string()
                    .contains("non-finite sample")
            );
            drop(writer);
            assert_eq!(std::fs::read(&wav).unwrap(), b"keep");
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
        for kind in [
            WAV_FAULT_WRITE,
            WAV_FAULT_FLUSH,
            WAV_FAULT_RENAME,
            WAV_FAULT_DIR_SYNC,
        ] {
            std::fs::write(&wav, b"keep").unwrap();
            let mut writer =
                WavFile::create_with_dither(&wav, 48_000, 2, format, Dither::Tpdf).unwrap();
            writer.write_block(&[0.0, 0.0]).unwrap();
            arm_wav_fault(kind, 0);
            let error = writer
                .write_block(&[0.0, 0.0])
                .and_then(|()| writer.finish())
                .unwrap_err();
            assert_eq!(error.is_durability_unknown(), kind == WAV_FAULT_DIR_SYNC);
            let bytes = std::fs::read(&wav).unwrap();
            if kind == WAV_FAULT_DIR_SYNC {
                assert_eq!(&bytes[..4], b"RIFF");
            } else {
                assert_eq!(bytes, b"keep");
            }
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "timing measurement; make spike-render runs it in a release build"]
fn a_minute_of_pcm_with_and_without_tpdf_is_written_in_bulk() {
    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let wav = directory.join("minute.wav");
    let frames = 48_000u32 * 60;
    // This stereo block is measurement scratch, not an export capacity limit.
    let block = [0.25; 256 * 2];
    for (format, name, header) in [
        (WavFormat::Pcm16, "pcm16", 44),
        (WavFormat::Pcm24, "pcm24", 68),
    ] {
        for (dither, label) in [(Dither::None, "none"), (Dither::Tpdf, "tpdf")] {
            let started = std::time::Instant::now();
            let mut writer =
                WavFile::create_with_dither(&wav, 48_000, frames, format, dither).unwrap();
            let mut remaining = frames as usize;
            while remaining > 0 {
                let count = remaining.min(block.len() / 2);
                writer.write_block(&block[..count * 2]).unwrap();
                remaining -= count;
            }
            writer.finish().unwrap();
            let elapsed = started.elapsed();
            assert_eq!(
                std::fs::metadata(&wav).unwrap().len(),
                header + u64::from(frames) * u64::from(format.frame_bytes())
            );
            eprintln!("60 s stereo {name}, dither={label}: {elapsed:?}");
            assert!(
                elapsed.as_secs_f64() < 2.0,
                "60 s {name} {label} write took {elapsed:?}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
