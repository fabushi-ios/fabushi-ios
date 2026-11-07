use gpui_telegram_media::range_stream::{ByteRange, RangeError, RangeStreamAdapter};

#[test]
fn test_closed_range_parsing() {
    let range = RangeStreamAdapter::parse_range_header("bytes=0-499", 1000).unwrap();
    assert_eq!(range, ByteRange { start: 0, end: 499 });
    assert_eq!(range.length(), 500);
}

#[test]
fn test_open_ended_range_parsing() {
    let range = RangeStreamAdapter::parse_range_header("bytes=1000-", 2048).unwrap();
    assert_eq!(range, ByteRange { start: 1000, end: 2047 });
    assert_eq!(range.length(), 1048);
}

#[test]
fn test_suffix_range_parsing() {
    let range = RangeStreamAdapter::parse_range_header("bytes=-500", 2048).unwrap();
    assert_eq!(range, ByteRange { start: 1548, end: 2047 });
    assert_eq!(range.length(), 500);
}

#[test]
fn test_invalid_range_handling() {
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=3000-4000", 2048),
        Err(RangeError::NotSatisfiable(2048))
    );
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=500-200", 2048),
        Err(RangeError::NotSatisfiable(2048))
    );
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=-0", 2048),
        Err(RangeError::NotSatisfiable(2048))
    );
}

#[test]
fn test_header_formatting() {
    let range = ByteRange { start: 0, end: 499 };
    assert_eq!(
        RangeStreamAdapter::format_content_range(&range, 2048),
        "bytes 0-499/2048"
    );
    assert_eq!(
        RangeStreamAdapter::format_unsatisfiable_content_range(2048),
        "bytes */2048"
    );
}

#[test]
fn test_slice_extraction() {
    let bytes: Vec<u8> = (0..100).collect();
    let range = ByteRange { start: 5, end: 15 };
    let slice = RangeStreamAdapter::extract_slice(&bytes, &range).unwrap();
    assert_eq!(slice.len(), 11);
    assert_eq!(slice[0], 5);
    assert_eq!(slice[10], 15);
}
