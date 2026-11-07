use gpui_telegram_media::chunked_upload::{recommend_part_size, ChunkedUploadSession};
use gpui_telegram_media::range_stream::{ByteRange, RangeError, RangeStreamAdapter};
use gpui_telegram_media::thumbnails::{
    fit_dimensions_downscale_only, shrink_to_keep_aspect, ThumbnailGenerator, ThumbnailPreset,
};
use gpui_telegram_media::waveform::{
    decode_waveform_5bit, decode_waveform_to_floats, encode_waveform_5bit,
    encode_waveform_from_floats, normalize_pcm_peaks, resample_and_normalize_waveform,
    TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES,
};
use sha2::{Digest, Sha256};

// =========================================================================
// 1. 5-Bit Waveform Codec Tests
// =========================================================================

#[test]
fn test_waveform_roundtrip_100_bars() {
    // Generate 100 known values spanning 0..=31
    let original: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
        .map(|i| (i % 32) as u8)
        .collect();

    let encoded = encode_waveform_5bit(&original);
    assert_eq!(
        encoded.len(),
        TELEGRAM_WAVEFORM_BYTES,
        "100 bars must pack to exactly 63 bytes"
    );

    let decoded = decode_waveform_5bit(&encoded);
    assert_eq!(
        decoded.len(),
        100,
        "Decoded bars count must equal 100"
    );
    assert_eq!(
        decoded, original,
        "Lossless roundtrip: decoded bars must match original"
    );
}

#[test]
fn test_waveform_boundary_cases() {
    // Empty bars
    assert!(encode_waveform_5bit(&[]).is_empty());
    assert!(decode_waveform_5bit(&[]).is_empty());

    // Single bar
    let single = vec![17u8];
    let encoded_single = encode_waveform_5bit(&single);
    assert_eq!(encoded_single.len(), 1);
    let decoded_single = decode_waveform_5bit(&encoded_single);
    assert_eq!(decoded_single, vec![17u8]);

    // All zeros (silence)
    let zeros = vec![0u8; 100];
    let encoded_zeros = encode_waveform_5bit(&zeros);
    assert_eq!(encoded_zeros.len(), 63);
    assert!(encoded_zeros.iter().all(|&b| b == 0));
    assert_eq!(decode_waveform_5bit(&encoded_zeros), zeros);

    // All 31s (maximum saturation)
    let max_bars = vec![31u8; 100];
    let encoded_max = encode_waveform_5bit(&max_bars);
    assert_eq!(encoded_max.len(), 63);
    assert_eq!(decode_waveform_5bit(&encoded_max), max_bars);

    // Values > 31 clamped to 5 bits (0x1F)
    let unclamped = vec![255u8; 100];
    let encoded_unclamped = encode_waveform_5bit(&unclamped);
    assert_eq!(decode_waveform_5bit(&encoded_unclamped), vec![31u8; 100]);
}

#[test]
fn test_waveform_float_conversion() {
    let float_peaks: Vec<f32> = (0..100).map(|i| (i as f32) / 100.0).collect();
    let packed = encode_waveform_from_floats(&float_peaks);
    assert_eq!(packed.len(), 63);

    let unpacked = decode_waveform_to_floats(&packed, 100);
    assert_eq!(unpacked.len(), 100);

    for (orig, dec) in float_peaks.iter().zip(unpacked.iter()) {
        assert!(
            (orig - dec).abs() <= 0.05,
            "Float quantization tolerance exceeded: orig={}, dec={}",
            orig,
            dec
        );
    }
}

#[test]
fn test_pcm_peak_normalization_and_ceiling() {
    // Quiet audio (below floor of 2500)
    let quiet_peaks = vec![100u16; 50];
    let bars = normalize_pcm_peaks(&quiet_peaks);
    assert_eq!(bars.len(), 50);
    // 100 * 31 / 2500 = 1
    assert_eq!(bars[0], 1);

    // Dynamic ceiling compression with transient burst
    let mut dynamic_peaks = vec![5000u16; 99];
    dynamic_peaks.push(65535); // loud transient cough/spike
    let normalized = normalize_pcm_peaks(&dynamic_peaks);
    assert_eq!(normalized.len(), 100);
    // Transient clamped to max 31 without flattening rest
    assert!(normalized[0] >= 15);
    assert_eq!(normalized[99], 31);
}

#[test]
fn test_waveform_resampler() {
    let raw_samples: Vec<u8> = (0..500).map(|i| (i % 256) as u8).collect();
    let bars = resample_and_normalize_waveform(&raw_samples);
    assert_eq!(bars.len(), 100);
}

// =========================================================================
// 2. Chunked Media Upload Tests
// =========================================================================

#[test]
fn test_chunked_upload_out_of_order_receipt() {
    // 1000 bytes with 300 byte part_size -> 4 parts: [300, 300, 300, 100]
    let total_size = 1000u64;
    let part_size = 300u32;
    let mut session = ChunkedUploadSession::new("test_file_001", total_size, part_size).unwrap();

    assert_eq!(session.total_parts, 4);
    assert_eq!(session.expected_part_size(0), 300);
    assert_eq!(session.expected_part_size(1), 300);
    assert_eq!(session.expected_part_size(2), 300);
    assert_eq!(session.expected_part_size(3), 100);
    assert_eq!(session.missing_parts(), vec![0, 1, 2, 3]);

    let part0 = vec![10u8; 300];
    let part1 = vec![20u8; 300];
    let part2 = vec![30u8; 300];
    let part3 = vec![40u8; 100];

    // Receive Part 2 first (out of order)
    session.receive_part(2, &part2).unwrap();
    assert!(session.is_part_received(2));
    assert_eq!(session.missing_parts(), vec![0, 1, 3]);
    assert!(!session.is_complete());

    // Receive Part 0
    session.receive_part(0, &part0).unwrap();
    assert_eq!(session.missing_parts(), vec![1, 3]);

    // Duplicate receive Part 0 (idempotent)
    session.receive_part(0, &part0).unwrap();
    assert_eq!(session.missing_parts(), vec![1, 3]);

    // Receive Part 3
    session.receive_part(3, &part3).unwrap();
    assert_eq!(session.missing_parts(), vec![1]);

    // Receive Part 1 (final piece)
    session.receive_part(1, &part1).unwrap();
    assert!(session.missing_parts().is_empty());
    assert!(session.is_complete());

    // Assemble and verify payload
    let assembled = session.assemble_payload().unwrap();
    assert_eq!(assembled.len(), 1000);
    assert_eq!(&assembled[0..300], &part0[..]);
    assert_eq!(&assembled[300..600], &part1[..]);
    assert_eq!(&assembled[600..900], &part2[..]);
    assert_eq!(&assembled[900..1000], &part3[..]);
}

#[test]
fn test_chunked_upload_sha256_validation() {
    let payload = b"The quick brown fox jumps over the lazy dog resilient media upload";
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let expected_sha256 = format!("{:x}", hasher.finalize());

    let mut session =
        ChunkedUploadSession::new("fox_upload", payload.len() as u64, 16)
            .unwrap()
            .with_expected_sha256(&expected_sha256);

    let parts: Vec<&[u8]> = payload.chunks(16).collect();
    for (idx, &chunk) in parts.iter().enumerate() {
        session.receive_part(idx as u32, chunk).unwrap();
    }

    assert!(session.is_complete());
    assert!(session.finalize_and_verify().unwrap());
    assert_eq!(session.compute_sha256().unwrap(), expected_sha256);
}

#[test]
fn test_chunked_upload_pause_and_resume_serialization() {
    let mut session = ChunkedUploadSession::new("pause_resume_doc", 500, 100).unwrap();
    session.receive_part(0, &vec![1u8; 100]).unwrap();
    session.receive_part(2, &vec![3u8; 100]).unwrap();

    // Serialize to JSON to simulate app backgrounding / restart
    let serialized = serde_json::to_string(&session).unwrap();
    let mut restored: ChunkedUploadSession = serde_json::from_str(&serialized).unwrap();

    assert_eq!(restored.missing_parts(), vec![1, 3, 4]);
    assert_eq!(restored.file_id, "pause_resume_doc");

    // Ingest remaining parts
    restored.receive_part(1, &vec![2u8; 100]).unwrap();
    restored.receive_part(3, &vec![4u8; 100]).unwrap();
    restored.receive_part(4, &vec![5u8; 100]).unwrap();

    assert!(restored.is_complete());
    let payload = restored.assemble_payload().unwrap();
    assert_eq!(payload.len(), 500);
}

#[test]
fn test_chunked_upload_recommended_part_sizes() {
    assert_eq!(recommend_part_size(500 * 1024), 32 * 1024);
    assert_eq!(recommend_part_size(10 * 1024 * 1024), 64 * 1024);
    assert_eq!(recommend_part_size(50 * 1024 * 1024), 128 * 1024);
    assert_eq!(recommend_part_size(500 * 1024 * 1024), 256 * 1024);
    assert_eq!(recommend_part_size(1000 * 1024 * 1024), 512 * 1024);
    assert_eq!(recommend_part_size(3000 * 1024 * 1024), 1024 * 1024);
}

// =========================================================================
// 3. HTTP 206 Range Streaming Adapter Tests
// =========================================================================

#[test]
fn test_range_stream_closed_range() {
    let total_size = 2048u64;
    let range = RangeStreamAdapter::parse_range_header("bytes=0-1023", total_size).unwrap();
    assert_eq!(range, ByteRange { start: 0, end: 1023 });
    assert_eq!(range.length(), 1024);
    assert_eq!(
        RangeStreamAdapter::format_content_range(&range, total_size),
        "bytes 0-1023/2048"
    );

    // Range exceeding EOF clamped to total_size - 1
    let clamped = RangeStreamAdapter::parse_range_header("bytes=1500-3000", total_size).unwrap();
    assert_eq!(clamped, ByteRange { start: 1500, end: 2047 });
}

#[test]
fn test_range_stream_open_ended_range() {
    let total_size = 2048u64;
    let range = RangeStreamAdapter::parse_range_header("bytes=1024-", total_size).unwrap();
    assert_eq!(range, ByteRange { start: 1024, end: 2047 });
    assert_eq!(range.length(), 1024);
}

#[test]
fn test_range_stream_suffix_range() {
    let total_size = 2048u64;
    // Last 500 bytes
    let range = RangeStreamAdapter::parse_range_header("bytes=-500", total_size).unwrap();
    assert_eq!(range, ByteRange { start: 1548, end: 2047 });
    assert_eq!(range.length(), 500);

    // Suffix larger than entire file starts at 0
    let large_suffix = RangeStreamAdapter::parse_range_header("bytes=-3000", total_size).unwrap();
    assert_eq!(large_suffix, ByteRange { start: 0, end: 2047 });
}

#[test]
fn test_range_stream_out_of_bounds_and_invalid() {
    let total_size = 2048u64;

    // Start >= total_size
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=3000-4000", total_size),
        Err(RangeError::NotSatisfiable(2048))
    );

    // Inverted range start > end
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=1000-500", total_size),
        Err(RangeError::NotSatisfiable(2048))
    );

    // Suffix 0
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=-0", total_size),
        Err(RangeError::NotSatisfiable(2048))
    );

    // Empty file size
    assert_eq!(
        RangeStreamAdapter::parse_range_header("bytes=0-10", 0),
        Err(RangeError::NotSatisfiable(0))
    );

    // Missing bytes= prefix
    assert_eq!(
        RangeStreamAdapter::parse_range_header("items=0-10", total_size),
        Err(RangeError::MissingPrefix)
    );
}

#[test]
fn test_range_stream_slice_extraction() {
    let data: Vec<u8> = (0..100).collect();
    let range = ByteRange { start: 10, end: 19 };
    let slice = RangeStreamAdapter::extract_slice(&data, &range).unwrap();
    assert_eq!(slice.len(), 10);
    assert_eq!(slice[0], 10);
    assert_eq!(slice[9], 19);
}

// =========================================================================
// 4. Thumbnail Geometry & Pipeline Tests
// =========================================================================

#[test]
fn test_shrink_to_keep_aspect_upstream_cross_multiplication() {
    // 16:9 1920x1080 scaled into 320x320 bounding box -> (320, 180)
    let (w, h) = shrink_to_keep_aspect(1920, 1080, 320, 320);
    assert_eq!((w, h), (320, 180));

    // 9:16 1080x1920 scaled into 320x320 bounding box -> (180, 320)
    let (w, h) = shrink_to_keep_aspect(1080, 1920, 320, 320);
    assert_eq!((w, h), (180, 320));

    // 1:1 1000x1000 scaled into 320x320 bounding box -> (320, 320)
    let (w, h) = shrink_to_keep_aspect(1000, 1000, 320, 320);
    assert_eq!((w, h), (320, 320));

    // Preset tests for 1920x1080
    assert_eq!(
        ThumbnailGenerator::fit_preset(1920, 1080, ThumbnailPreset::Micro),
        (32, 18)
    );
    assert_eq!(
        ThumbnailGenerator::fit_preset(1920, 1080, ThumbnailPreset::Small),
        (100, 56)
    );
    assert_eq!(
        ThumbnailGenerator::fit_preset(1920, 1080, ThumbnailPreset::Medium),
        (320, 180)
    );
    assert_eq!(
        ThumbnailGenerator::fit_preset(1920, 1080, ThumbnailPreset::Large),
        (800, 450)
    );
    assert_eq!(
        ThumbnailGenerator::fit_preset(1920, 1080, ThumbnailPreset::ExtraLarge),
        (1280, 720)
    );
}

#[test]
fn test_thumbnail_fit_dimensions_downscale_only() {
    // Image smaller than box retains original dimensions
    let (w, h) = fit_dimensions_downscale_only(200, 150, 320, 320);
    assert_eq!((w, h), (200, 150));

    // Image larger in one dimension downscales
    let (w, h) = fit_dimensions_downscale_only(640, 200, 320, 320);
    assert_eq!((w, h), (320, 100));
}

#[test]
fn test_real_image_thumbnail_generation() {
    // Construct a valid 100x50 in-memory image using the image crate
    let img = image::RgbImage::new(100, 50);
    let mut png_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png_bytes);
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();

    let thumb_bytes =
        ThumbnailGenerator::generate_thumbnail(&png_bytes, ThumbnailPreset::Micro).unwrap();
    assert!(!thumb_bytes.is_empty());

    // Verify generated JPEG is decoded with correct resized dimensions (32, 16)
    let decoded = image::load_from_memory(&thumb_bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (32, 16));
}
