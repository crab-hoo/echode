//! Open items from docs/reviews/2026-11-03-full-code-audit.md. Each test fails
//! until its item is fixed; `make known-bugs` runs them. Remove the `#[ignore]`
//! with the fix.

use echode_cli::write_wav;

fn chunk_size(bytes: &[u8], id: &[u8; 4]) -> Option<u32> {
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
        if &bytes[at..at + 4] == id {
            return Some(size);
        }
        at += 8 + size as usize + (size as usize & 1);
    }
    None
}

/// FCA-16: an IEEE float WAV carries the 18-byte format chunk (with `cbSize`)
/// and a `fact` chunk, which strict readers expect for a non-PCM format. The
/// fix also moves every expected header length in `render.rs` (44 bytes today).
#[test]
#[ignore = "FCA-16: the float WAV has a 16-byte fmt chunk and no fact chunk"]
fn audit_fca16_a_float_wav_has_an_extended_format_and_a_fact_chunk() {
    let path = std::env::temp_dir().join(format!("echode-fca16-{}.wav", std::process::id()));
    write_wav(&path, 48_000, &[0.0; 8]).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(
        chunk_size(&bytes, b"fmt "),
        Some(18),
        "the float format chunk has no cbSize field"
    );
    assert_eq!(
        chunk_size(&bytes, b"fact"),
        Some(4),
        "the float WAV has no fact chunk"
    );
}
