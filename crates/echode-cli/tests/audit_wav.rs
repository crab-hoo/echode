//! Float WAV metadata and sample preservation regressions for FCA-16.

mod support;

use echode_cli::write_wav;
use echode_model::Id;

fn exported(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("echode-fca16-{}.wav", Id::generate()));
    write_wav(&path, sample_rate, samples).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    bytes
}

/// FCA-16: non-PCM WAV needs WAVEFORMATEX's cbSize and a fact sample count.
#[test]
fn audit_fca16_a_float_wav_has_an_extended_format_and_a_fact_chunk() {
    let bytes = exported(48_000, &[0.0; 8]);
    let format = support::chunk(&bytes, b"fmt ");
    assert_eq!(
        format.len(),
        18,
        "the float format chunk has no cbSize field"
    );
    assert_eq!(&format[16..18], &0u16.to_le_bytes());
    assert_eq!(support::chunk(&bytes, b"fact"), &4u32.to_le_bytes());
    assert_eq!(support::chunk(&bytes, b"data"), &[0; 32]);
}

#[test]
fn float_wav_metadata_and_samples_survive_empty_short_and_multiblock_writes() {
    // These lengths bound the regression fixtures only, not product exports.
    for frames in [0, 1, 129] {
        let samples: Vec<f32> = [0.0, -0.0, 0.25, -0.5, 2.0, -2.0]
            .into_iter()
            .cycle()
            .take(frames * 2)
            .collect();
        let expected: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        for sample_rate in [44_100u32, 48_000, 96_000] {
            let bytes = exported(sample_rate, &samples);
            let format = support::chunk(&bytes, b"fmt ");
            assert_eq!(format.len(), 18);
            assert_eq!(&format[..4], &[3, 0, 2, 0]); // IEEE float, stereo.
            assert_eq!(&format[4..8], &sample_rate.to_le_bytes());
            assert_eq!(&format[8..12], &(sample_rate * 8).to_le_bytes());
            assert_eq!(&format[12..], &[8, 0, 32, 0, 0, 0]); // Alignment, bits, cbSize.
            assert_eq!(
                support::chunk(&bytes, b"fact"),
                &(frames as u32).to_le_bytes()
            );
            assert_eq!(support::chunk(&bytes, b"data"), expected);
            assert_eq!(bytes.len(), 58 + expected.len());
        }
    }
}
