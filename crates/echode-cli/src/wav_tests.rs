//! Header-only boundary checks avoid allocating a multi-gigabyte WAV fixture.

use super::wav_header;

#[test]
fn riff_size_includes_the_extended_format_and_fact_chunks_at_the_limit() {
    let frames = (u32::MAX - 50) / 8;
    let header = wav_header(48_000, frames).unwrap();
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
            wav_header(48_000, too_many)
                .unwrap_err()
                .to_string()
                .contains("wav data does not fit in a 32-bit chunk")
        );
    }
}

#[test]
fn byte_rate_must_fit_in_the_header() {
    let sample_rate = u32::MAX / 8;
    let header = wav_header(sample_rate, 0).unwrap();
    assert_eq!(
        u32::from_le_bytes(header[28..32].try_into().unwrap()),
        sample_rate * 8
    );
    assert!(
        wav_header(sample_rate + 1, 0)
            .unwrap_err()
            .to_string()
            .contains("sample rate does not fit in the wav header")
    );
}
