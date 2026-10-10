pub const ASCII_MARKERS: [(usize, &str); 4] = [
    (4, "ftyp"),
    (0, "OggS"),
    (0, "FLV"),
    (8, "AVI "),
];

pub const BYTE_MARKERS: [&[u8]; 4] = [
    &[26, 69, 223, 163],
    &[48, 38, 178, 117],
    &[0, 0, 1, 186],
    &[0, 0, 1, 179],
];

pub fn has_ascii_at(bytes: &[u8], offset: usize, value: &str) -> bool {
    let source = value.as_bytes();
    bytes.len() >= offset + source.len() && bytes[offset..offset + source.len()] == *source
}

pub fn bytes_look_like_video_container(bytes: &[u8]) -> bool {
    ASCII_MARKERS
        .iter()
        .any(|(offset, text)| has_ascii_at(bytes, *offset, text))
        || BYTE_MARKERS.iter().any(|marker| bytes.starts_with(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_reference_container_markers() {
        assert!(bytes_look_like_video_container(b"\0\0\0\0ftyp"));
        assert!(bytes_look_like_video_container(b"OggSrest"));
        assert!(bytes_look_like_video_container(&[26, 69, 223, 163, 1]));
        assert!(!bytes_look_like_video_container(b"plain text"));
    }
}
