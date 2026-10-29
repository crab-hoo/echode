//! Stored PCM is untrusted even when its content hash is correct.

use std::path::PathBuf;

use echode_audio_io::load_placed_clips;
use echode_engine::PlaybackError;
use echode_history::{read_object, read_pcm, write_object, write_package};
use echode_model::{AudioRef, Id, Project, Tick};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("echode-pcm-validation-{}", Id::generate()));
        write_package(&path, &Project::new()).unwrap();
        Self(path)
    }

    fn store(&self, channels: u16, samples: &[f32]) -> String {
        let mut bytes = b"ECHDPCM1".to_vec();
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&(samples.len() as u64 / u64::from(channels)).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        write_object(&self.0, &bytes).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn contract_stored_pcm_rejects_nonfinite_samples_before_clip_preparation() {
    let scratch = Scratch::new();
    assert_finite_samples(&scratch);
    for channels in [1u16, 2] {
        // Sixteen frames bound only this fixture, not accepted media lengths.
        let mut samples = vec![0.25; 16 * usize::from(channels)];
        for bits in [
            0x7fc1_2345, // Quiet NaN, both signs and a non-default payload.
            0xffc1_2345,
            0x7f80_0001, // Signaling NaN, both signs.
            0xff80_0001,
            0x7f80_0000, // Positive and negative infinity.
            0xff80_0000,
        ] {
            for index in 0..samples.len() {
                samples[index] = f32::from_bits(bits);
                let hash = scratch.store(channels, &samples);
                let original = read_object(&scratch.0, &hash).unwrap();
                let result = read_pcm(&scratch.0, &hash);
                assert!(
                    result.is_err(),
                    "canonical PCM must reject non-finite samples: {bits:08x}, {channels}ch, sample {index}"
                );
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "audio contains a non-finite sample"
                );

                let (project, track) = Project::new().add_track("Untrusted audio");
                let clip = Id::generate();
                let project = project
                    .place_audio(
                        track,
                        clip,
                        Id::generate(),
                        Tick::new(0),
                        AudioRef::try_new(hash.clone(), 48_000, channels, 16).unwrap(),
                    )
                    .unwrap();
                // Even when the bad sample lies outside the trimmed window,
                // the stored source must fail before SRC or plan publication.
                let length = project.tracks()[0].clips[0].length.get() / 2;
                let project = project
                    .resize_clip(track, clip, Tick::new(0), Tick::new(length))
                    .unwrap();
                project.check_invariants().unwrap();
                for rate in [48_000, 96_000] {
                    assert!(
                        matches!(
                            load_placed_clips(&scratch.0, &project, rate),
                            Err(PlaybackError::Media)
                        ),
                        "clip preparation must reject non-finite PCM: {bits:08x}, {channels}ch, sample {index}, rate {rate}"
                    );
                }
                // Refusal must not rewrite or repair the content-addressed object.
                assert_eq!(read_object(&scratch.0, &hash).unwrap(), original);
                samples[index] = 0.25;
            }
        }
    }
}

fn assert_finite_samples(scratch: &Scratch) {
    let samples = [
        0.0,
        -0.0,
        f32::from_bits(1),
        -f32::from_bits(1),
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        2.0,
        -2.0,
        f32::MAX,
        f32::MIN,
    ];
    for channels in [1u16, 2] {
        let hash = scratch.store(channels, &samples);
        let pcm = read_pcm(&scratch.0, &hash).unwrap();
        assert_eq!(pcm.sample_rate, 48_000);
        assert_eq!(pcm.channels, channels);
        assert_eq!(pcm.frames, samples.len() as u64 / u64::from(channels));
        assert_eq!(
            pcm.samples
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            samples.map(f32::to_bits),
            "finite PCM must retain exact bits, including values outside the nominal audio range"
        );
    }
}
