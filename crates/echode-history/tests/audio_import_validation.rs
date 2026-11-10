//! External audio must be validated before an import writes package data.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use echode_history::{
    AudioFileReader, DocumentHost, HostError, MediaError, PackageError, decode_audio_file,
    import_audio, read_pcm, write_package,
};
use echode_model::{Id, Project};
use serde_json::json;

const NONFINITE: &str = "audio contains a non-finite sample";
// These counts exercise the bulk decoder's read boundary, not a product limit.
const FRAMES: usize = 16_386;

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    package: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("echode-import-validation-{}", Id::generate()));
        let package = root.join("Song.echode");
        write_package(&package, &Project::new()).unwrap();
        Self {
            source: root.join("source.wav"),
            package,
            root,
        }
    }

    fn write_source(&self, samples: &[f32], channels: u16) -> Vec<u8> {
        let data = u32::try_from(samples.len() * 4).unwrap();
        let width = channels * 4;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float, not integer PCM.
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&(44_100 * u32::from(width)).to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(&self.source, &bytes).unwrap();
        bytes
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn package_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    collect(root, root, &mut files);
    files
}

fn reject_nonfinite_sources() {
    let fixture = Fixture::new();
    let before = package_files(&fixture.package);
    for channels in [1u16, 2] {
        let count = FRAMES * usize::from(channels);
        for bits in [
            0x7fc0_0001,
            0xffc0_1234,
            0x7f80_0001,
            0x7f80_0000,
            0xff80_0000,
        ] {
            for index in [0, count / 2, count - 1] {
                let mut samples = vec![0.25; count];
                samples[index] = f32::from_bits(bits);
                let original = fixture.write_source(&samples, channels);
                let imported = import_audio(&fixture.package, &fixture.source);
                assert!(
                    imported.is_err(),
                    "audio import must reject non-finite samples before writing: {bits:x}, {channels} channels, sample {index}"
                );
                let error = imported.unwrap_err();
                assert!(matches!(
                    error,
                    PackageError::Audio(MediaError::NonFiniteSample)
                ));
                assert_eq!(error.to_string(), NONFINITE);
                assert_eq!(
                    package_files(&fixture.package),
                    before,
                    "rejected import must not write objects, an index, peaks, or ignore rules"
                );
                assert_eq!(
                    decode_audio_file(&fixture.source, &|| true)
                        .unwrap_err()
                        .to_string(),
                    NONFINITE
                );
                let mut reader = AudioFileReader::open(&fixture.source).unwrap();
                loop {
                    match reader.read(16_384) {
                        Ok(block) => {
                            assert_ne!(block.frames, 0, "streaming decode must reject too")
                        }
                        Err(error) => {
                            assert_eq!(error.to_string(), NONFINITE);
                            break;
                        }
                    }
                }
                assert_eq!(fs::read(&fixture.source).unwrap(), original);
            }
        }
    }
}

fn failed_transaction_preserves_redo() {
    let fixture = Fixture::new();
    let mut samples = vec![0.25; FRAMES * 2];
    *samples.last_mut().unwrap() = f32::INFINITY;
    let original = fixture.write_source(&samples, 2);
    let host = DocumentHost::open(&fixture.package).unwrap();
    host.apply(vec![json!({"type": "track.add", "name": "redo me"})])
        .unwrap();
    let edited = host.snapshot();
    host.undo().unwrap();
    let before = host.snapshot();
    let result = host.apply(vec![
        json!({"type": "track.add", "name": "must roll back"}),
        json!({"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": fixture.source}),
    ]);
    assert!(
        matches!(result, Err(HostError::Element { index: 1, ref error }) if !error.is_parameters() && error.to_string() == NONFINITE),
        "non-finite import must reject the whole transaction: {result:?}"
    );
    assert_eq!(*host.snapshot(), *before);
    host.redo().unwrap();
    assert_eq!(
        *host.snapshot(),
        *edited,
        "failed import must preserve redo"
    );
    host.undo().unwrap();
    assert_eq!(
        *host.snapshot(),
        *before,
        "failed import must not add an undo step"
    );
    host.redo().unwrap();
    drop(host);
    let reopened = DocumentHost::open(&fixture.package).unwrap();
    assert_eq!(*reopened.snapshot(), *edited);
    drop(reopened);
    assert_eq!(fs::read(&fixture.source).unwrap(), original);
}

fn finite_samples_roundtrip() {
    let fixture = Fixture::new();
    for channels in [1u16, 2] {
        let values = [
            0.0,
            -0.0,
            f32::from_bits(1),
            -f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            1.25,
            -2.5,
        ];
        let samples: Vec<_> = values
            .into_iter()
            .cycle()
            .take(FRAMES * usize::from(channels))
            .collect();
        let original = fixture.write_source(&samples, channels);
        let decoded = decode_audio_file(&fixture.source, &|| true).unwrap();
        let stored = import_audio(&fixture.package, &fixture.source).unwrap();
        let loaded = read_pcm(&fixture.package, &stored.hash).unwrap();
        for pcm in [decoded, loaded] {
            assert_eq!(
                (pcm.sample_rate, pcm.channels, pcm.frames),
                (44_100, channels, FRAMES as u64)
            );
            assert_eq!(
                pcm.samples
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
                samples
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
                "finite imported samples must preserve their bits, without clipping or sanitizing"
            );
        }
        let before = package_files(&fixture.package);
        assert_eq!(
            import_audio(&fixture.package, &fixture.source).unwrap(),
            stored
        );
        assert_eq!(
            package_files(&fixture.package),
            before,
            "duplicate import must stay byte-stable"
        );
        assert_eq!(fs::read(&fixture.source).unwrap(), original);

        // A rejected import must also preserve an existing index and valid media.
        let mut invalid = samples;
        *invalid.last_mut().unwrap() = f32::NAN;
        let original = fixture.write_source(&invalid, channels);
        assert_eq!(
            import_audio(&fixture.package, &fixture.source)
                .unwrap_err()
                .to_string(),
            NONFINITE
        );
        assert_eq!(package_files(&fixture.package), before);
        assert_eq!(fs::read(&fixture.source).unwrap(), original);
    }
    assert_eq!(
        decode_audio_file(&fixture.source, &|| false)
            .unwrap_err()
            .to_string(),
        "audio decoding was cancelled"
    );
}

#[test]
fn contract_audio_import_rejects_nonfinite_before_writing_and_preserves_history() {
    reject_nonfinite_sources();
    failed_transaction_preserves_redo();
    finite_samples_roundtrip();
}

/// Objects, temporaries, the media index, and peaks.
fn media_files(package: &Path) -> Vec<PathBuf> {
    package_files(package)
        .into_keys()
        .filter(|path| path.starts_with("media") || path.starts_with("cache"))
        .collect()
}

/// EXT-02 (docs/reviews/2026-11-06-assessment-review.md): an import stored its
/// media before the placement was checked, and nothing removed the object when
/// the placement or its transaction was rejected. Imports are now staged and
/// published only with an accepted edit.
#[test]
fn audit_ext02_a_rejected_import_leaves_no_media_in_the_package() {
    let fixture = Fixture::new();
    let other = fixture.root.join("other.wav");
    fixture.write_source(&vec![0.5; FRAMES * 2], 2);
    fs::copy(&fixture.source, &other).unwrap();
    fixture.write_source(&vec![0.25; FRAMES * 2], 2);
    let host = DocumentHost::open(&fixture.package).unwrap();
    let before = host.snapshot();
    let media = media_files(&fixture.package);
    let missing = Id::generate();
    let placed = host.apply(vec![
        json!({"type": "audio.import", "track": missing, "start": 0, "path": fixture.source}),
    ]);
    assert!(
        placed.is_err(),
        "an import into a missing track was accepted"
    );
    let rolled_back = host.apply(vec![
        json!({"type": "track.add", "name": "rolled back"}),
        json!({"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": other}),
        json!({"type": "audio.import", "track": missing, "start": 0, "path": fixture.source}),
    ]);
    assert!(
        rolled_back.is_err(),
        "a transaction with a missing track was accepted"
    );
    assert_eq!(*host.snapshot(), *before);
    drop(host);
    assert_eq!(
        media_files(&fixture.package),
        media,
        "rejected imports left media in the package"
    );
}
