pub const ASCII_MARKERS: &[(usize, &str)] = &[(4, "ftyp"), (0, "OggS"), (0, "FLV"), (8, "AVI ")];
pub const BYTE_MARKERS: &[&[u8]] = &[&[26, 69, 223, 163], &[48, 38, 178, 117], &[0, 0, 1, 186], &[0, 0, 1, 179]];

pub fn has_ascii_at(bytes: &[u8], offset: usize, value: &str) -> bool {
    let end = offset.saturating_add(value.len());
    bytes.get(offset..end).is_some_and(|slice| slice == value.as_bytes())
}

pub fn bytes_look_like_video_container(bytes: &[u8]) -> bool {
    ASCII_MARKERS.iter().any(|(offset, text)| has_ascii_at(bytes, *offset, text))
        || BYTE_MARKERS.iter().any(|marker| bytes.starts_with(marker))
}
