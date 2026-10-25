//! Decode an audio file and store the samples by their BLAKE3 hash.
//!
//! The object is canonical PCM at the file's own rate. Playback resamples it
//! when the device rate differs. Ogg stays out of this slice: AudioToolbox
//! covers WAV, AIFF, CAF, MP3, AAC, ALAC, and FLAC.

#[cfg(test)]
use std::fs;
use std::path::Path;

use objc2_audio_toolbox::{
    ExtAudioFileDispose, ExtAudioFileGetProperty, ExtAudioFileOpenURL, ExtAudioFileRead,
    ExtAudioFileRef, ExtAudioFileSeek, ExtAudioFileSetProperty,
    kExtAudioFileProperty_ClientDataFormat, kExtAudioFileProperty_FileDataFormat,
    kExtAudioFileProperty_FileLengthFrames,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, kAudioFormatFlagsNativeFloatPacked,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFIndex, CFURL};

use crate::document::PackageError;
use crate::objects::{ensure_store_gitignore, read_object, write_object};

pub(crate) const PCM_MAGIC: &[u8; 8] = b"ECHDPCM1";
const MAGIC: &[u8; 8] = PCM_MAGIC;
const HEADER: usize = 24;

/// What an import stored. The project records these facts on the clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAudio {
    pub hash: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
}

/// Interleaved float samples read back from the object store.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedPcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub samples: Vec<f32>,
}

/// Decodes `source` on the control thread and stores the canonical PCM blob.
///
/// The same bytes are not written twice. The index keeps the first file name
/// for a hash.
pub fn import_audio(package: &Path, source: &Path) -> Result<StoredAudio, PackageError> {
    let decoded = decode_file(source, &|| true)?;
    let bytes = encode_pcm(
        decoded.sample_rate,
        decoded.channels,
        decoded.frames,
        &decoded.samples,
    )?;
    ensure_store_gitignore(package)?;
    let hash = write_object(package, &bytes)?;
    remember_name(package, &hash, source, &decoded)?;
    let _ = crate::peaks::cache_peaks(package, &hash);
    Ok(StoredAudio {
        hash,
        sample_rate: decoded.sample_rate,
        channels: decoded.channels,
        frames: decoded.frames,
    })
}

/// Reads a blob written by [`import_audio`].
pub fn read_pcm(package: &Path, hash: &str) -> Result<DecodedPcm, PackageError> {
    decode_pcm(&read_object(package, hash)?)
}

pub(crate) fn encode_pcm(
    sample_rate: u32,
    channels: u16,
    frames: u64,
    samples: &[f32],
) -> Result<Vec<u8>, PackageError> {
    let count = frames
        .checked_mul(u64::from(channels))
        .ok_or_else(|| PackageError::new("audio frame count overflowed"))?;
    if u64::try_from(samples.len()).ok() != Some(count) {
        return Err(PackageError::new("audio sample count does not match"));
    }
    let mut bytes = Vec::with_capacity(HEADER + samples.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&frames.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

fn decode_pcm(bytes: &[u8]) -> Result<DecodedPcm, PackageError> {
    if bytes.len() < HEADER || &bytes[..8] != MAGIC {
        return Err(PackageError::new("audio object is not canonical PCM"));
    }
    let sample_rate = u32::from_le_bytes(bytes[8..12].try_into().expect("four rate bytes"));
    let channels = u16::from_le_bytes(bytes[12..14].try_into().expect("two channel bytes"));
    let reserved = u16::from_le_bytes(bytes[14..16].try_into().expect("two reserved bytes"));
    let frames = u64::from_le_bytes(bytes[16..24].try_into().expect("eight frame bytes"));
    if reserved != 0 || sample_rate == 0 || !matches!(channels, 1 | 2) || frames == 0 {
        return Err(PackageError::new("audio object header is unusable"));
    }
    let count = frames
        .checked_mul(u64::from(channels))
        .ok_or_else(|| PackageError::new("audio frame count overflowed"))?;
    let count = usize::try_from(count).map_err(|_| PackageError::new("audio is too large"))?;
    let expected = count
        .checked_mul(4)
        .and_then(|payload| HEADER.checked_add(payload))
        .ok_or_else(|| PackageError::new("audio byte count overflowed"))?;
    if bytes.len() != expected {
        return Err(PackageError::new("audio object length does not match"));
    }
    let payload = &bytes[HEADER..];
    let mut samples = Vec::with_capacity(count);
    for chunk in payload.chunks_exact(4) {
        samples.push(f32::from_le_bytes(
            chunk.try_into().expect("four sample bytes"),
        ));
    }
    Ok(DecodedPcm {
        sample_rate,
        channels,
        frames,
        samples,
    })
}

/// Decodes `source` at its own rate without storing it, for a sample audition.
/// Reading stops with an error once `keep_going` returns false.
pub fn decode_audio_file(
    source: &Path,
    keep_going: &dyn Fn() -> bool,
) -> Result<DecodedPcm, PackageError> {
    decode_file(source, keep_going)
}

/// A control-thread decoder with bounded reads. It owns the AudioToolbox file
/// and must stay on the thread that opened it. No project or cache is written.
pub struct AudioFileReader {
    file: OpenAudio,
    sample_rate: u32,
    channels: u16,
    /// The file's reported length; reads still stop at the actual EOF.
    frames: u64,
}

impl AudioFileReader {
    /// The next read begins at this source sample, including inside a packet.
    /// EOF is a valid seek; a position beyond the file is refused.
    pub fn seek(&mut self, frame: u64) -> Result<(), PackageError> {
        let offset =
            i64::try_from(frame).map_err(|_| PackageError::new("audio seek overflowed"))?;
        if frame > self.frames {
            return Err(PackageError::new("audio seek exceeds the file"));
        }
        // SAFETY: this reader owns the valid file, on the opening thread.
        let status = unsafe { ExtAudioFileSeek(self.file.0, offset) };
        if status != 0 {
            return Err(PackageError::new(format!(
                "could not seek audio ({status})"
            )));
        }
        Ok(())
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn open(source: &Path) -> Result<Self, PackageError> {
        let file = open_audio(source)?;
        let format = audio_property::<AudioStreamBasicDescription>(
            file.0,
            kExtAudioFileProperty_FileDataFormat,
        )?;
        if !matches!(format.mChannelsPerFrame, 1 | 2) {
            return Err(PackageError::new(
                "only mono and stereo audio can be imported",
            ));
        }
        let sample_rate = integer_rate(format.mSampleRate)?;
        let frames = audio_property::<i64>(file.0, kExtAudioFileProperty_FileLengthFrames)?;
        if frames <= 0 {
            return Err(PackageError::new("audio file has no samples"));
        }
        set_audio_property(
            file.0,
            kExtAudioFileProperty_ClientDataFormat,
            &client_format(format.mSampleRate, format.mChannelsPerFrame),
        )?;
        Ok(Self {
            file,
            sample_rate,
            channels: format.mChannelsPerFrame as u16,
            frames: frames as u64,
        })
    }

    /// Reads at most `frames` frames at the source rate. An empty result is EOF.
    pub fn read(&mut self, frames: u32) -> Result<DecodedPcm, PackageError> {
        if frames == 0 {
            return Err(PackageError::new("audio read must request frames"));
        }
        let bytes = frames
            .checked_mul(u32::from(self.channels))
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| PackageError::new("audio read is too large"))?;
        let mut samples = vec![0.0f32; bytes as usize / 4];
        let mut got = frames;
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: u32::from(self.channels),
                mDataByteSize: bytes,
                mData: samples.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: the buffer holds the requested number of interleaved frames.
        let status = unsafe {
            ExtAudioFileRead(
                self.file.0,
                core::ptr::NonNull::from(&mut got),
                core::ptr::NonNull::from(&mut list),
            )
        };
        if status != 0 {
            return Err(PackageError::new(format!(
                "could not read audio ({status})"
            )));
        }
        if got > frames {
            return Err(PackageError::new("audio decoder exceeded its buffer"));
        }
        samples.truncate(got as usize * usize::from(self.channels));
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(PackageError::new("audio contains a non-finite sample"));
        }
        Ok(DecodedPcm {
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames: u64::from(got),
            samples,
        })
    }
}

fn decode_file(source: &Path, keep_going: &dyn Fn() -> bool) -> Result<DecodedPcm, PackageError> {
    let file = open_audio(source)?;
    let file_format = audio_property::<AudioStreamBasicDescription>(
        file.0,
        kExtAudioFileProperty_FileDataFormat,
    )?;
    let channels = file_format.mChannelsPerFrame;
    if !matches!(channels, 1 | 2) {
        return Err(PackageError::new(
            "only mono and stereo audio can be imported",
        ));
    }
    let sample_rate = integer_rate(file_format.mSampleRate)?;
    let client = client_format(file_format.mSampleRate, channels);
    set_audio_property(file.0, kExtAudioFileProperty_ClientDataFormat, &client)?;
    let frames = audio_property::<i64>(file.0, kExtAudioFileProperty_FileLengthFrames)?;
    if frames <= 0 {
        return Err(PackageError::new("audio file has no samples"));
    }
    let frames = u64::try_from(frames).map_err(|_| PackageError::new("audio is too large"))?;
    let channels_usize = channels as usize;
    let count = frames
        .checked_mul(channels_usize as u64)
        .ok_or_else(|| PackageError::new("audio frame count overflowed"))?;
    let count = usize::try_from(count).map_err(|_| PackageError::new("audio is too large"))?;
    let mut samples = vec![0.0; count];
    let mut written = 0u64;
    while written < frames {
        if !keep_going() {
            return Err(PackageError::new("audio decoding was cancelled"));
        }
        let ask = u32::try_from((frames - written).min(16_384)).unwrap_or(u32::MAX);
        let mut got = ask;
        let offset = usize::try_from(written)
            .map_err(|_| PackageError::new("audio is too large"))?
            * channels_usize;
        let byte_size = usize::try_from(ask).unwrap_or(0) * channels_usize * 4;
        let mut list = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: channels,
                mDataByteSize: u32::try_from(byte_size)
                    .map_err(|_| PackageError::new("audio read is too large"))?,
                mData: samples[offset..].as_mut_ptr().cast(),
            }],
        };
        let status = unsafe {
            ExtAudioFileRead(
                file.0,
                core::ptr::NonNull::from(&mut got),
                core::ptr::NonNull::from(&mut list),
            )
        };
        if status != 0 {
            return Err(PackageError::new(format!(
                "could not read audio ({status})"
            )));
        }
        if got == 0 {
            break;
        }
        written += u64::from(got);
    }
    if written == 0 {
        return Err(PackageError::new("audio file has no samples"));
    }
    let frames = written;
    let count = usize::try_from(frames).unwrap_or(0) * channels_usize;
    samples.truncate(count);
    Ok(DecodedPcm {
        sample_rate,
        channels: u16::try_from(channels).expect("channels are 1 or 2"),
        frames,
        samples,
    })
}

struct OpenAudio(ExtAudioFileRef);

impl Drop for OpenAudio {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ExtAudioFileDispose(self.0);
            }
        }
    }
}

fn open_audio(source: &Path) -> Result<OpenAudio, PackageError> {
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(source.as_os_str());
    let url = unsafe {
        CFURL::from_file_system_representation(
            None,
            bytes.as_ptr(),
            CFIndex::try_from(bytes.len()).unwrap_or(CFIndex::MAX),
            false,
        )
    }
    .ok_or_else(|| PackageError::new("audio path is not a file URL"))?;
    let mut opened: ExtAudioFileRef = core::ptr::null_mut();
    let status =
        unsafe { ExtAudioFileOpenURL(&url, core::ptr::NonNull::new_unchecked(&mut opened)) };
    if status != 0 || opened.is_null() {
        return Err(PackageError::new(format!(
            "could not open audio ({status})"
        )));
    }
    Ok(OpenAudio(opened))
}

fn integer_rate(rate: f64) -> Result<u32, PackageError> {
    if !rate.is_finite() || rate <= 0.0 || rate.fract() != 0.0 {
        return Err(PackageError::new(
            "audio sample rate is not a positive integer",
        ));
    }
    u32::try_from(rate as u64).map_err(|_| PackageError::new("audio sample rate is too high"))
}

fn client_format(sample_rate: f64, channels: u32) -> AudioStreamBasicDescription {
    let bytes = channels.saturating_mul(4);
    AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
        mBytesPerPacket: bytes,
        mFramesPerPacket: 1,
        mBytesPerFrame: bytes,
        mChannelsPerFrame: channels,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

fn audio_property<T>(file: ExtAudioFileRef, property: u32) -> Result<T, PackageError> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX);
    let status = unsafe {
        ExtAudioFileGetProperty(
            file,
            property,
            core::ptr::NonNull::from(&mut size),
            core::ptr::NonNull::new_unchecked(value.as_mut_ptr()).cast(),
        )
    };
    if status != 0 || size as usize != std::mem::size_of::<T>() {
        return Err(PackageError::new(format!(
            "could not read an audio property ({status})"
        )));
    }
    Ok(unsafe { value.assume_init() })
}

fn set_audio_property<T>(
    file: ExtAudioFileRef,
    property: u32,
    value: &T,
) -> Result<(), PackageError> {
    let size = u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX);
    let status = unsafe {
        ExtAudioFileSetProperty(file, property, size, core::ptr::NonNull::from(value).cast())
    };
    if status != 0 {
        return Err(PackageError::new(format!(
            "could not set the audio client format ({status})"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct MediaIndex {
    files: Vec<MediaEntry>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct MediaEntry {
    hash: String,
    name: String,
    format: String,
    sample_rate: u32,
    channels: u16,
    frames: u64,
}

fn remember_name(
    package: &Path,
    hash: &str,
    source: &Path,
    audio: &DecodedPcm,
) -> Result<(), PackageError> {
    let directory = package.join("media");
    let owned = crate::package_io::Directory::open(package)
        .and_then(|root| root.child("media"))
        .map_err(|error| PackageError::new(format!("{}: {error}", directory.display())))?;
    let path = directory.join("index.json");
    let mut index = if owned
        .has_file("index.json")
        .map_err(|error| PackageError::new(error.to_string()))?
    {
        let text = owned
            .read_text("index.json")
            .map_err(|error| PackageError::new(format!("{}: {error}", path.display())))?;
        serde_json::from_str::<MediaIndex>(&text)
            .map_err(|error| PackageError::new(format!("media index: {error}")))?
    } else {
        MediaIndex { files: Vec::new() }
    };
    if !index.files.iter().any(|entry| entry.hash == hash) {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "audio".to_string());
        let format = source
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .filter(|extension| !extension.is_empty())
            .unwrap_or_else(|| "audio".to_string());
        index.files.push(MediaEntry {
            hash: hash.to_string(),
            name,
            format,
            sample_rate: audio.sample_rate,
            channels: audio.channels,
            frames: audio.frames,
        });
        index
            .files
            .sort_by(|left, right| left.hash.cmp(&right.hash));
    }
    let text = serde_json::to_string_pretty(&index)
        .map_err(|error| PackageError::new(format!("media index: {error}")))?;
    owned
        .write_atomic("index.json", format!("{text}\n").as_bytes(), false)
        .map_err(|error| PackageError::new(format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_mono(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_bytes = samples.len() * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&u32::try_from(36 + data_bytes).unwrap().to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&u32::try_from(data_bytes).unwrap().to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    #[test]
    fn a_wav_is_stored_once_and_reads_back() {
        let root = std::env::temp_dir().join(format!("echode-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("tone.wav");
        fs::write(&source, wav_mono(48_000, &[0, 16_384, -16_384])).unwrap();
        let stored = import_audio(&root, &source).unwrap();
        assert_eq!(stored.sample_rate, 48_000);
        assert_eq!(stored.channels, 1);
        assert_eq!(stored.frames, 3);
        let copy = root.join("other-name.wav");
        fs::write(&copy, wav_mono(48_000, &[0, 16_384, -16_384])).unwrap();
        let again = import_audio(&root, &copy).unwrap();
        assert_eq!(again.hash, stored.hash);
        let index = fs::read_to_string(root.join("media/index.json")).unwrap();
        assert_eq!(index.matches(&stored.hash).count(), 1);
        assert!(index.contains("tone.wav"));
        let pcm = read_pcm(&root, &stored.hash).unwrap();
        assert!((pcm.samples[1] - 0.5).abs() < 1.0e-3);
        assert!((pcm.samples[2] + 0.5).abs() < 1.0e-3);
        let _ = fs::remove_dir_all(root);
    }
}
