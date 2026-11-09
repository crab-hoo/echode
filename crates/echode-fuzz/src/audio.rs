//! Audio the product reads: stored PCM objects (`read_pcm`) and files a user
//! imports or auditions (`import_audio`, `decode_audio_file`, `AudioFileReader`).
//!
//! Input: one kind byte, then the bytes. Kind 0 stores the bytes as a PCM
//! object. Other kinds write them to a file with one of [`EXTENSIONS`], which
//! AudioToolbox uses to pick a parser.

use std::path::Path;

use echode_history::{
    AudioFileReader, DecodedPcm, decode_audio_file, import_audio, read_pcm, write_object,
};

use crate::sample;
use crate::scratch::Scratch;

const EXTENSIONS: [&str; 7] = ["wav", "aif", "caf", "mp3", "m4a", "flac", "aac"];

/// More reads are not made. Bounds this target's run time only.
const MAX_READS: usize = 256;
const READ_FRAMES: u32 = 4_096;

pub(crate) fn run(data: &[u8]) {
    let Some((&kind, bytes)) = data.split_first() else {
        return;
    };
    let kind = usize::from(kind) % (EXTENSIONS.len() + 1);
    let scratch = Scratch::new("audio");
    let package = scratch.path().join("song.echode");
    match kind.checked_sub(1).and_then(|index| EXTENSIONS.get(index)) {
        None => stored(&package, bytes),
        Some(extension) => {
            std::fs::create_dir_all(scratch.path()).expect("create the scratch folder");
            let source = scratch.path().join(format!("input.{extension}"));
            std::fs::write(&source, bytes).expect("write the audio file");
            file(&package, &source);
        }
    }
}

fn stored(package: &Path, bytes: &[u8]) {
    let hash = write_object(package, bytes).expect("store the object");
    if let Ok(pcm) = read_pcm(package, &hash) {
        assert_pcm(&pcm, "a stored object");
    }
}

fn file(package: &Path, source: &Path) {
    let decoded = decode_audio_file(source, &|| true);
    if let Ok(pcm) = &decoded {
        assert_pcm(pcm, "a decoded file");
    }
    match import_audio(package, source) {
        Ok(stored) => {
            let pcm = read_pcm(package, &stored.hash).expect("an imported object reads back");
            assert_pcm(&pcm, "an imported object");
            assert_eq!(
                (stored.sample_rate, stored.channels, stored.frames),
                (pcm.sample_rate, pcm.channels, pcm.frames),
                "the import reported another shape than it stored"
            );
            assert!(
                decoded.as_ref().is_ok_and(|decoded| *decoded == pcm),
                "the import stored other samples than the file decodes to"
            );
        }
        Err(_) => assert!(decoded.is_err(), "a file that decodes was not imported"),
    }
    if let Ok(mut reader) = AudioFileReader::open(source) {
        assert!(matches!(reader.channels(), 1 | 2));
        assert!(reader.sample_rate() > 0);
        for _ in 0..MAX_READS {
            match reader.read(READ_FRAMES) {
                Ok(pcm) if pcm.frames == 0 => break,
                Ok(pcm) => {
                    assert!(pcm.frames <= u64::from(READ_FRAMES));
                    assert_eq!(pcm.channels, reader.channels());
                    assert_pcm(&pcm, "a bounded read");
                }
                Err(_) => break,
            }
        }
    }
}

fn assert_pcm(pcm: &DecodedPcm, what: &str) {
    assert!(
        matches!(pcm.channels, 1 | 2),
        "{what} has {} channels",
        pcm.channels
    );
    assert!(pcm.sample_rate > 0, "{what} has no sample rate");
    assert_eq!(
        u64::try_from(pcm.samples.len()).ok(),
        pcm.frames.checked_mul(u64::from(pcm.channels)),
        "{what} has another sample count than its frames say"
    );
    assert!(
        pcm.samples.iter().all(|sample| sample.is_finite()),
        "{what} has a non-finite sample"
    );
}

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let wav = sample::wav_pcm16(48_000, 2, 480);
    let mut seeds = vec![
        kind("wav", &wav),
        kind("wav", &sample::wav_pcm16(44_100, 1, 64)),
        kind("wav", &sample::wav_float(48_000, 2, 128)),
        kind("aif", &sample::aiff_pcm16(2, 96)),
    ];
    // The canonical object an import stores.
    let scratch = Scratch::new("audio-seed");
    let source = scratch.path().join("seed.wav");
    std::fs::create_dir_all(scratch.path()).expect("create the scratch folder");
    std::fs::write(&source, &wav).expect("write the seed");
    let package = scratch.path().join("song.echode");
    let stored = import_audio(&package, &source).expect("import the seed");
    let object = std::fs::read(package.join("media").join("objects").join(&stored.hash))
        .expect("read the stored object");
    let mut pcm = vec![0];
    pcm.extend_from_slice(&object);
    seeds.push(pcm);
    seeds
}

fn kind(extension: &str, bytes: &[u8]) -> Vec<u8> {
    let index = EXTENSIONS
        .iter()
        .position(|candidate| *candidate == extension)
        .expect("a known extension");
    let mut seed = vec![index as u8 + 1];
    seed.extend_from_slice(bytes);
    seed
}
