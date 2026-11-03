//! RF64 boundaries and sparse product-writer fixtures, without gigabytes of PCM.

use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;

use super::wav_tests::WRITER;
use super::{WavFile, WavFormat, wav_header};
use echode_history::AudioFileReader;

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[test]
fn rf64_replaces_the_riff_size_limit_for_every_export_format() {
    for (format, riff_header, rf64_header, bits, subtype) in [
        (WavFormat::Float32, 58u32, 116usize, 32u16, 3u32),
        (WavFormat::Pcm16, 44, 104, 16, 1),
        (WavFormat::Pcm24, 68, 104, 24, 1),
    ] {
        let frame_bytes = u32::from(format.frame_bytes());
        let last_riff = (u32::MAX - (riff_header - 8)) / frame_bytes;
        let header = wav_header(48_000, last_riff, format).unwrap();
        assert_eq!(&header[..4], b"RIFF");
        assert_eq!(header.len(), riff_header as usize);
        assert_eq!(
            u32_at(&header, 4),
            last_riff * frame_bytes + (riff_header - 8)
        );

        // Include metadata-only overflow, data overflow, and the renderer's u32 frame limit.
        for frames in [last_riff + 1, u32::MAX / frame_bytes + 1, u32::MAX] {
            let header = wav_header(48_000, frames, format).unwrap_or_else(|error| {
                panic!("RF64 export must replace the RIFF size limit: {error}")
            });
            let data_bytes = u64::from(frames) * u64::from(frame_bytes);
            assert_eq!(header.len(), rf64_header);
            assert_eq!(&header[..4], b"RF64");
            assert_eq!(u32_at(&header, 4), u32::MAX);
            assert_eq!(&header[8..16], b"WAVEds64");
            assert_eq!(u32_at(&header, 16), 28);
            assert_eq!(u64_at(&header, 20), data_bytes + rf64_header as u64 - 8);
            assert_eq!(u64_at(&header, 28), data_bytes);
            assert_eq!(u64_at(&header, 36), u64::from(frames));
            assert_eq!(u32_at(&header, 44), 0); // No additional large chunks.
            assert_eq!(&header[48..52], b"fmt ");
            assert_eq!(u32_at(&header, 52), 40);
            assert_eq!(&header[56..60], &[0xfe, 0xff, 2, 0]);
            assert_eq!(u32_at(&header, 60), 48_000);
            assert_eq!(u32_at(&header, 64), 48_000 * frame_bytes);
            assert_eq!(&header[68..70], &(frame_bytes as u16).to_le_bytes());
            assert_eq!(&header[70..72], &bits.to_le_bytes());
            assert_eq!(&header[72..74], &22u16.to_le_bytes());
            assert_eq!(&header[74..76], &bits.to_le_bytes());
            assert_eq!(u32_at(&header, 76), 3); // Front left/right.
            assert_eq!(u32_at(&header, 80), subtype);
            assert_eq!(
                &header[84..96],
                &[0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]
            );
            if subtype == 3 {
                assert_eq!(&header[96..100], b"fact");
                assert_eq!(u32_at(&header, 100), 4);
                assert_eq!(u32_at(&header, 104), u32::MAX);
            }
            assert_eq!(&header[rf64_header - 8..rf64_header - 4], b"data");
            assert_eq!(u32_at(&header, rf64_header - 4), u32::MAX);
        }
    }
}

#[test]
fn rf64_sparse_exports_decode_and_seek_past_four_gibibytes() {
    let _guard = WRITER.lock().unwrap();
    let directory =
        std::env::temp_dir().join(format!("echode-rf64-{}", echode_model::Id::generate()));
    std::fs::create_dir_all(&directory).unwrap();
    for format in [WavFormat::Float32, WavFormat::Pcm16, WavFormat::Pcm24] {
        let destination = directory.join("large.wav");
        let frames = u32::MAX / u32::from(format.frame_bytes()) + 2;
        let header = wav_header(48_000, frames, format).unwrap();
        let mut wav = sparse_wav(&destination, frames, format);
        wav.write_block(&[-0.75, 0.125]).unwrap();
        wav.finish().unwrap();
        let metadata = std::fs::metadata(&destination).unwrap();
        assert_eq!(metadata.len(), u64_at(&header, 20) + 8);
        // This bound is fixture-only: fail rather than silently allocate gigabytes of disk space.
        assert!(metadata.blocks() * 512 < 1024 * 1024);
        let mut published = vec![0; header.len()];
        std::fs::File::open(&destination)
            .unwrap()
            .read_exact(&mut published)
            .unwrap();
        assert_eq!(published, header);

        let mut reader = AudioFileReader::open(&destination).unwrap();
        assert_eq!(reader.sample_rate(), 48_000);
        assert_eq!(reader.channels(), 2);
        assert_eq!(reader.frames(), u64::from(frames));
        assert_eq!(reader.read(1).unwrap().samples, [0.25, -0.5]);
        reader.seek(u64::from(frames / 2)).unwrap();
        assert_eq!(reader.read(1).unwrap().samples, [0.0, 0.0]);
        reader.seek(u64::from(frames - 1)).unwrap();
        assert_eq!(reader.read(1).unwrap().samples, [-0.75, 0.125]);
        assert!(reader.read(1).unwrap().samples.is_empty());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

// Skip only the fixture's silent middle, leaving the final frame for the caller.
fn sparse_wav(destination: &std::path::Path, frames: u32, format: WavFormat) -> WavFile {
    let header = wav_header(48_000, frames, format).unwrap();
    let mut wav = WavFile::create(destination, 48_000, frames, format).unwrap();
    wav.write_block(&[0.25, -0.5]).unwrap();
    let last = header.len() as u64 + u64::from(frames - 1) * u64::from(format.frame_bytes());
    wav.writer.seek(SeekFrom::Start(last)).unwrap();
    wav
}

#[test]
fn rf64_keeps_atomic_publication_on_drop_write_flush_rename_and_sync_failures() {
    use super::{
        WAV_FAULT_DIR_SYNC, WAV_FAULT_FLUSH, WAV_FAULT_RENAME, WAV_FAULT_WRITE, arm_wav_fault,
    };

    let _guard = WRITER.lock().unwrap();
    let directory = std::env::temp_dir().join(format!(
        "echode-rf64-faults-{}",
        echode_model::Id::generate()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let destination = directory.join("keep.wav");
    for format in [WavFormat::Float32, WavFormat::Pcm16, WavFormat::Pcm24] {
        let frames = u32::MAX / u32::from(format.frame_bytes()) + 2;
        std::fs::write(&destination, b"keep").unwrap();
        drop(sparse_wav(&destination, frames, format));
        assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        for fault in [
            WAV_FAULT_WRITE,
            WAV_FAULT_FLUSH,
            WAV_FAULT_RENAME,
            WAV_FAULT_DIR_SYNC,
        ] {
            std::fs::write(&destination, b"keep").unwrap();
            let mut wav = sparse_wav(&destination, frames, format);
            arm_wav_fault(fault, 0);
            let error = wav
                .write_block(&[-0.75, 0.125])
                .and_then(|()| wav.finish())
                .unwrap_err();
            assert_eq!(error.is_durability_unknown(), fault == WAV_FAULT_DIR_SYNC);
            if fault == WAV_FAULT_DIR_SYNC {
                let header = wav_header(48_000, frames, format).unwrap();
                assert_eq!(
                    std::fs::metadata(&destination).unwrap().len(),
                    u64_at(&header, 20) + 8
                );
                let mut reader = AudioFileReader::open(&destination).unwrap();
                reader.seek(u64::from(frames - 1)).unwrap();
                assert_eq!(reader.read(1).unwrap().samples, [-0.75, 0.125]);
            } else {
                assert_eq!(std::fs::read(&destination).unwrap(), b"keep");
            }
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        }
        let error = WavFile::create(&destination, u32::MAX, frames, format)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "sample rate does not fit in the wav header"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
