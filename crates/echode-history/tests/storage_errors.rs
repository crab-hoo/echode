//! Storage failures retain their causes instead of only their display text.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use echode_history::{
    AudioFileReader, MediaError, PackageError, decode_audio_file, import_audio, object_hash,
    read_object, read_package, read_pcm, write_object,
};
use echode_model::Id;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("echode-storage-errors-{}", Id::generate()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn wav(&self) -> PathBuf {
        // One mono PCM frame bounds only this storage-error fixture.
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&38u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0i16.to_le_bytes());
        let source = self.0.join("source.wav");
        fs::write(&source, bytes).unwrap();
        source
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn package_and_object_reads_retain_io_causes() {
    let fixture = Fixture::new();
    for error in [
        read_package(&fixture.0).unwrap_err(),
        read_object(&fixture.0, &object_hash(b"missing")).unwrap_err(),
    ] {
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<std::io::Error>());
        assert!(
            source.is_some(),
            "storage error lost its I/O cause: {error:?}"
        );
        assert_eq!(source.unwrap().kind(), std::io::ErrorKind::NotFound);
        assert!(
            error
                .to_string()
                .starts_with(&fixture.0.display().to_string())
        );
    }
}

#[test]
fn import_retains_a_malformed_index_cause_and_withdraws_the_object() {
    let fixture = Fixture::new();
    let source = fixture.wav();
    fs::create_dir_all(fixture.0.join("media")).unwrap();
    fs::write(fixture.0.join("media/index.json"), b"{").unwrap();
    let error = import_audio(&fixture.0, &source).unwrap_err();
    let cause = error
        .source()
        .and_then(|source| source.downcast_ref::<serde_json::Error>());
    assert!(
        cause.is_some(),
        "media index error lost its JSON cause: {error:?}"
    );
    assert!(cause.unwrap().is_eof());
    assert!(error.to_string().starts_with("media index: "));
    assert_eq!(fs::read(fixture.0.join("media/index.json")).unwrap(), b"{");
    assert_eq!(
        fs::read_dir(fixture.0.join("media/objects"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn object_validation_is_distinct_from_storage_io() {
    let fixture = Fixture::new();
    assert!(matches!(
        read_object(&fixture.0, "not-a-hash"),
        Err(PackageError::InvalidObjectHash)
    ));
    let hash = write_object(&fixture.0, b"original").unwrap();
    fs::write(fixture.0.join("media/objects").join(&hash), b"damaged").unwrap();
    let error = read_object(&fixture.0, &hash).unwrap_err();
    assert!(matches!(
        &error,
        PackageError::ObjectHashMismatch { hash: found } if found == &hash
    ));
    assert_eq!(
        error.to_string(),
        format!("object {hash} does not match its BLAKE3")
    );
    assert!(error.source().is_none());
}

#[test]
fn canonical_pcm_validation_retains_the_failure_kind() {
    let fixture = Fixture::new();
    // A single-frame object bounds this validation matrix, not product audio.
    let canonical = || {
        let mut bytes = b"ECHDPCM1".to_vec();
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0.25f32.to_le_bytes());
        bytes
    };
    let mut header = canonical();
    header[14] = 1;
    let mut frame_overflow = canonical();
    frame_overflow[12..14].copy_from_slice(&2u16.to_le_bytes());
    frame_overflow[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut byte_overflow = canonical();
    byte_overflow[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut nonfinite = canonical();
    nonfinite[24..28].copy_from_slice(&f32::NAN.to_le_bytes());
    for (bytes, expected) in [
        (vec![], MediaError::NotCanonicalPcm),
        (header, MediaError::UnusableHeader),
        (canonical()[..24].to_vec(), MediaError::LengthMismatch),
        (frame_overflow, MediaError::FrameCountOverflow),
        (byte_overflow, MediaError::ByteCountOverflow),
        (nonfinite, MediaError::NonFiniteSample),
    ] {
        let hash = write_object(&fixture.0, &bytes).unwrap();
        let error = read_pcm(&fixture.0, &hash).unwrap_err();
        assert_eq!(error.to_string(), expected.to_string());
        let PackageError::Audio(actual) = &error else {
            panic!("PCM validation lost its typed cause: {error:?}");
        };
        assert_eq!(
            std::mem::discriminant(actual),
            std::mem::discriminant(&expected)
        );
        assert!(error.source().unwrap().is::<MediaError>());
    }
    let hash = write_object(&fixture.0, &canonical()).unwrap();
    assert_eq!(read_pcm(&fixture.0, &hash).unwrap().samples, [0.25]);
}

#[test]
fn decoder_rejections_and_native_failures_remain_distinct() {
    let fixture = Fixture::new();
    let error = AudioFileReader::open(&fixture.0.join("missing.wav"))
        .err()
        .unwrap();
    let PackageError::Audio(MediaError::Open(status)) = error else {
        panic!("native decoder status was lost: {error:?}");
    };
    assert_ne!(status, 0);
    let source = fixture.wav();
    assert!(matches!(
        decode_audio_file(&source, &|| false),
        Err(PackageError::Audio(MediaError::Cancelled))
    ));
    let mut reader = AudioFileReader::open(&source).unwrap();
    assert!(matches!(
        reader.read(0),
        Err(PackageError::Audio(MediaError::EmptyRead))
    ));
    assert!(matches!(
        reader.read(u32::MAX),
        Err(PackageError::Audio(MediaError::ReadTooLarge))
    ));
    assert!(matches!(
        reader.seek(u64::MAX),
        Err(PackageError::Audio(MediaError::SeekOverflow))
    ));
    assert!(matches!(
        reader.seek(2),
        Err(PackageError::Audio(MediaError::SeekPastEnd))
    ));
    reader.seek(1).unwrap();
    assert_eq!(reader.read(1).unwrap().frames, 0);
    reader.seek(0).unwrap();
    assert_eq!(reader.read(1).unwrap().frames, 1);
}
