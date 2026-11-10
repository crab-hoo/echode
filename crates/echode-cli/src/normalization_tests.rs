use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn output() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "echode-normalization-unit-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn encode(samples: &[f32], block: usize, format: WavFormat, dither: Dither) -> Vec<u8> {
    let path = output();
    let mut wav = ExportOutput::create(
        &path,
        48_000,
        (samples.len() / 2) as u32,
        format,
        dither,
        Some(PeakTarget::parse("-1").unwrap()),
    )
    .unwrap();
    for chunk in samples.chunks(block * 2) {
        wav.write_block(chunk).unwrap();
    }
    wav.finish_cancellable(&Cancellation::default()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

#[test]
fn gain_and_tpdf_are_independent_of_render_block_boundaries() {
    let _lock = crate::wav_tests::WRITER.lock().unwrap();
    // Only this fixture length is bounded; it spans spool and encoding buffers.
    let samples: Vec<_> = (0..20_002)
        .map(|n| ((n as f32) * 0.17).sin() * 2.0)
        .collect();
    for (format, dither) in [
        (WavFormat::Float32, Dither::None),
        (WavFormat::Pcm16, Dither::Tpdf),
        (WavFormat::Pcm24, Dither::Tpdf),
    ] {
        let expected = encode(&samples, 64, format, dither);
        for block in [1, 127, 256, 4096] {
            assert_eq!(encode(&samples, block, format, dither), expected);
        }
    }
}

#[test]
fn normalization_handles_subnormal_and_over_full_scale_samples() {
    let _lock = crate::wav_tests::WRITER.lock().unwrap();
    for peak in [f32::from_bits(1), 0.25, 4.0, f32::MAX] {
        let bytes = encode(&[peak, -peak], 1, WavFormat::Float32, Dither::None);
        let data = &bytes[bytes.len() - 8..];
        let left = f32::from_le_bytes(data[..4].try_into().unwrap());
        let right = f32::from_le_bytes(data[4..].try_into().unwrap());
        assert!((f64::from(left) - 10.0f64.powf(-1.0 / 20.0)).abs() < 1e-7);
        assert_eq!(right, -left);
    }
}

#[test]
fn invalid_samples_and_failed_spool_reads_keep_the_original_and_remove_the_partial() {
    let _lock = crate::wav_tests::WRITER.lock().unwrap();
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let path = output();
        std::fs::write(&path, b"original").unwrap();
        let mut wav = ExportOutput::create(
            &path,
            48_000,
            1,
            WavFormat::Float32,
            Dither::None,
            Some(PeakTarget::parse("0").unwrap()),
        )
        .unwrap();
        let temporary = wav.wav.temporary.clone();
        assert!(matches!(
            wav.write_block(&[value, 0.0]).unwrap_err(),
            CliError::NonFiniteNormalized
        ));
        drop(wav);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(!temporary.exists());
        std::fs::remove_file(path).unwrap();
    }
    let path = output();
    std::fs::write(&path, b"original").unwrap();
    let mut wav = ExportOutput::create(
        &path,
        48_000,
        1,
        WavFormat::Pcm24,
        Dither::None,
        Some(PeakTarget::parse("-1").unwrap()),
    )
    .unwrap();
    let temporary = wav.wav.temporary.clone();
    wav.write_block(&[0.25, -0.125]).unwrap();
    let spool = &mut wav.normalization.as_mut().unwrap().spool;
    spool.flush().unwrap();
    spool.get_ref().set_len(0).unwrap();
    let error = wav
        .finish_cancellable(&Cancellation::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("normalization scratch I/O failed")
    );
    assert!(matches!(&error, CliError::Io {
        operation: CliIoOperation::ReadScratch, error, ..
    } if error.kind() == std::io::ErrorKind::UnexpectedEof));
    assert!(std::error::Error::source(&error).is_some_and(|cause| cause.is::<std::io::Error>()));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert!(!temporary.exists());
    std::fs::remove_file(path).unwrap();
}
