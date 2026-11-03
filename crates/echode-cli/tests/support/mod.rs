//! Inspect RIFF chunks without assuming a particular WAV header length.

pub fn chunk<'a>(bytes: &'a [u8], id: &[u8; 4]) -> &'a [u8] {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    assert_eq!(u64::from(riff_size) + 8, bytes.len() as u64);
    let mut at = 12;
    let mut found = None;
    while at < bytes.len() {
        let header = bytes.get(at..at + 8).expect("truncated chunk header");
        let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let start = at + 8;
        let end = start.checked_add(size).expect("chunk size overflow");
        let data = bytes.get(start..end).expect("truncated chunk data");
        if &header[..4] == id {
            assert!(found.is_none(), "duplicate chunk {id:?}");
            found = Some(data);
        }
        at = end.checked_add(size & 1).expect("chunk padding overflow");
    }
    assert_eq!(at, bytes.len(), "truncated chunk padding");
    found.unwrap_or_else(|| panic!("missing chunk {id:?}"))
}
