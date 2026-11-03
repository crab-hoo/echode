//! Header-only boundary checks avoid allocating a multi-gigabyte WAV fixture.

use super::{WavFile, WavFormat, wav_header};

// Fault injection is process-wide; every file-writing test must hold this lock.
static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn riff_size_includes_the_extended_format_and_fact_chunks_at_the_limit() {
    let frames = (u32::MAX - 50) / 8;
    let header = wav_header(48_000, frames, WavFormat::Float32).unwrap();
    assert_eq!(header.len(), 58);
    assert_eq!(
        u32::from_le_bytes(header[4..8].try_into().unwrap()),
        frames * 8 + 50
    );
    assert_eq!(
        u32::from_le_bytes(header[46..50].try_into().unwrap()),
        frames
    );
    assert_eq!(
        u32::from_le_bytes(header[54..58].try_into().unwrap()),
        frames * 8
    );
    for too_many in [frames + 1, u32::MAX] {
        assert!(
            wav_header(48_000, too_many, WavFormat::Float32)
                .unwrap_err()
                .to_string()
                .contains("wav data does not fit in a 32-bit chunk")
        );
    }
}

#[test]
fn byte_rate_must_fit_in_the_header() {
    let sample_rate = u32::MAX / 8;
    let header = wav_header(sample_rate, 0, WavFormat::Float32).unwrap();
    assert_eq!(
        u32::from_le_bytes(header[28..32].try_into().unwrap()),
        sample_rate * 8
    );
    assert!(
        wav_header(sample_rate + 1, 0, WavFormat::Float32)
            .unwrap_err()
            .to_string()
            .contains("sample rate does not fit in the wav header")
    );
}

#[test]
fn pcm24_riff_and_byte_rate_limits_use_six_bytes_per_frame() {
    let frames = (u32::MAX - 60) / 6;
    let header = wav_header(48_000, frames, WavFormat::Pcm24).unwrap();
    assert_eq!(header.len(), 68);
    assert_eq!(
        u32::from_le_bytes(header[4..8].try_into().unwrap()),
        frames * 6 + 60
    );
    assert_eq!(
        u32::from_le_bytes(header[64..68].try_into().unwrap()),
        frames * 6
    );
    for too_many in [frames + 1, u32::MAX] {
        assert!(
            wav_header(48_000, too_many, WavFormat::Pcm24)
                .unwrap_err()
                .to_string()
                .contains("wav data does not fit in a 32-bit chunk")
        );
    }
    let rate = u32::MAX / 6;
    let header = wav_header(rate, 0, WavFormat::Pcm24).unwrap();
    assert_eq!(
        u32::from_le_bytes(header[28..32].try_into().unwrap()),
        rate * 6
    );
    assert!(
        wav_header(rate + 1, 0, WavFormat::Pcm24)
            .unwrap_err()
            .to_string()
            .contains("sample rate does not fit in the wav header")
    );
}

fn scratch() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "echode-pcm24-writer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn pcm24_rounds_half_steps_away_from_zero_and_saturates_without_wrapping() {
    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let destination = directory.join("out.wav");
    let lsb = 1.0 / 8_388_608.0;
    let samples = [
        -f32::MAX,
        -1.0,
        -0.5,
        -1.5 * lsb,
        -0.5 * lsb,
        -0.49 * lsb,
        -0.0,
        0.0,
        0.49 * lsb,
        0.5 * lsb,
        1.5 * lsb,
        0.5,
        1.0 - lsb,
        1.0,
        2.0,
        f32::MAX,
    ];
    let expected: [i32; 16] = [
        -8_388_608, -8_388_608, -4_194_304, -2, -1, 0, 0, 0, 0, 1, 2, 4_194_304, 8_388_607,
        8_388_607, 8_388_607, 8_388_607,
    ];
    let mut writer = WavFile::create(&destination, 48_000, 8, WavFormat::Pcm24).unwrap();
    writer.write_block(&samples).unwrap();
    writer.finish().unwrap();
    let bytes = std::fs::read(&destination).unwrap();
    let expected: Vec<_> = expected
        .iter()
        .flat_map(|sample| sample.to_le_bytes()[..3].to_vec())
        .collect();
    assert_eq!(&bytes[68..], expected);
    // Stateless conversion must not depend on how the writer splits the input.
    let mut writer = WavFile::create(&destination, 48_000, 8, WavFormat::Pcm24).unwrap();
    for block in samples.chunks(2) {
        writer.write_block(block).unwrap();
    }
    writer.finish().unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pcm24_nonfinite_samples_preserve_the_destination_and_remove_partial_files() {
    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let destination = directory.join("keep.wav");
    std::fs::write(&destination, b"keep").unwrap();
    for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut writer = WavFile::create(&destination, 48_000, 2, WavFormat::Pcm24).unwrap();
        writer.write_block(&[0.5, -0.5]).unwrap();
        let error = writer.write_block(&[0.0, sample]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("non-finite sample in PCM24 export")
        );
        drop(writer);
        assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pcm24_keeps_transactional_publication_on_write_flush_rename_and_sync_failures() {
    use super::{
        WAV_FAULT_DIR_SYNC, WAV_FAULT_FLUSH, WAV_FAULT_RENAME, WAV_FAULT_WRITE, arm_wav_fault,
    };

    let _guard = WRITER.lock().unwrap();
    let directory = scratch();
    let destination = directory.join("keep.wav");
    for kind in [
        WAV_FAULT_WRITE,
        WAV_FAULT_FLUSH,
        WAV_FAULT_RENAME,
        WAV_FAULT_DIR_SYNC,
    ] {
        std::fs::write(&destination, b"keep").unwrap();
        let mut writer = WavFile::create(&destination, 48_000, 2, WavFormat::Pcm24).unwrap();
        writer.write_block(&[0.5, -0.5]).unwrap();
        arm_wav_fault(kind, 0);
        let result = writer
            .write_block(&[0.0, 0.25])
            .and_then(|()| writer.finish());
        let error = result.unwrap_err();
        assert_eq!(error.is_durability_unknown(), kind == WAV_FAULT_DIR_SYNC);
        let bytes = std::fs::read(&destination).unwrap();
        if kind == WAV_FAULT_DIR_SYNC {
            assert_eq!(bytes.len(), 68 + 2 * 6);
            assert_eq!(&bytes[..4], b"RIFF");
        } else {
            assert_eq!(bytes, b"keep");
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
