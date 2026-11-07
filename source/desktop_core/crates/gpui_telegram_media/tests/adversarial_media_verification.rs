use gpui_telegram_core::error::TelegramError;
use gpui_telegram_media::chunked_upload::ChunkedUploadSession;
use gpui_telegram_media::range_stream::{ByteRange, RangeError, RangeStreamAdapter};
use gpui_telegram_media::waveform::decoder::decode_waveform_5bit;
use gpui_telegram_media::waveform::encoder::{
    encode_waveform_5bit, TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::thread;

// =========================================================================
// 1. 5-Bit Voice Waveform Codec Fuzzing & Stress Testing (TCG-N07)
// =========================================================================

struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xDEADBEEFCAFEBABE } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_bar(&mut self) -> u8 {
        (self.next_u64() % 32) as u8
    }
}

#[test]
fn test_fuzz_10000_waveform_roundtrips() {
    let mut rng = Xorshift64::new(0x2026_1106_CAFE_BABE);
    const ITERATIONS: usize = 10_000;

    for iter in 0..ITERATIONS {
        let mut original = [0u8; TELEGRAM_WAVEFORM_BARS];
        for bar in original.iter_mut() {
            *bar = rng.next_bar();
        }

        let encoded = encode_waveform_5bit(&original);
        assert_eq!(
            encoded.len(),
            TELEGRAM_WAVEFORM_BYTES,
            "Iteration {}: encoded length must be strictly {} bytes, got {}",
            iter,
            TELEGRAM_WAVEFORM_BYTES,
            encoded.len()
        );

        let decoded = decode_waveform_5bit(&encoded);
        assert_eq!(
            decoded.len(),
            TELEGRAM_WAVEFORM_BARS,
            "Iteration {}: decoded bar count must be strictly {}, got {}",
            iter,
            TELEGRAM_WAVEFORM_BARS,
            decoded.len()
        );

        assert_eq!(
            decoded.as_slice(),
            &original[..],
            "Iteration {}: bit-for-bit lossless equality failed!",
            iter
        );
    }
}

#[test]
fn test_waveform_extreme_edge_inputs() {
    // Edge case 1: All zeros
    let all_zeros = vec![0u8; TELEGRAM_WAVEFORM_BARS];
    let enc_zeros = encode_waveform_5bit(&all_zeros);
    assert_eq!(enc_zeros.len(), TELEGRAM_WAVEFORM_BYTES);
    assert!(enc_zeros.iter().all(|&b| b == 0));
    let dec_zeros = decode_waveform_5bit(&enc_zeros);
    assert_eq!(dec_zeros, all_zeros);

    // Edge case 2: All 31s (maximum 5-bit saturation)
    let all_31s = vec![31u8; TELEGRAM_WAVEFORM_BARS];
    let enc_31s = encode_waveform_5bit(&all_31s);
    assert_eq!(enc_31s.len(), TELEGRAM_WAVEFORM_BYTES);
    // 500 bits are 1s. 62 bytes * 8 = 496 bits (all 0xFF).
    for &b in &enc_31s[..62] {
        assert_eq!(b, 0xFF);
    }
    // The 63rd byte holds remaining 4 bits: 0x0F (unused high 4 bits are 0)
    assert_eq!(enc_31s[62], 0x0F);
    let dec_31s = decode_waveform_5bit(&enc_31s);
    assert_eq!(dec_31s, all_31s);

    // Edge case 3: Alternating 0 and 31
    let alt_0_31: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
        .map(|i| if i % 2 == 0 { 0 } else { 31 })
        .collect();
    let enc_alt = encode_waveform_5bit(&alt_0_31);
    assert_eq!(enc_alt.len(), TELEGRAM_WAVEFORM_BYTES);
    let dec_alt = decode_waveform_5bit(&enc_alt);
    assert_eq!(dec_alt, alt_0_31);

    // Edge case 4: Alternating 31 and 0
    let alt_31_0: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
        .map(|i| if i % 2 == 0 { 31 } else { 0 })
        .collect();
    let enc_alt2 = encode_waveform_5bit(&alt_31_0);
    assert_eq!(enc_alt2.len(), TELEGRAM_WAVEFORM_BYTES);
    let dec_alt2 = decode_waveform_5bit(&enc_alt2);
    assert_eq!(dec_alt2, alt_31_0);

    // Edge case 5: Walking single bit
    let walking_bit: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
        .map(|i| 1u8 << (i % 5))
        .collect();
    let enc_wb = encode_waveform_5bit(&walking_bit);
    assert_eq!(enc_wb.len(), TELEGRAM_WAVEFORM_BYTES);
    let dec_wb = decode_waveform_5bit(&enc_wb);
    assert_eq!(dec_wb, walking_bit);

    // Edge case 6: Monotonic ramp (i % 32)
    let ramp: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
        .map(|i| (i % 32) as u8)
        .collect();
    let enc_ramp = encode_waveform_5bit(&ramp);
    assert_eq!(enc_ramp.len(), TELEGRAM_WAVEFORM_BYTES);
    let dec_ramp = decode_waveform_5bit(&enc_ramp);
    assert_eq!(dec_ramp, ramp);
}

#[test]
fn test_waveform_buffer_length_invariants() {
    // Strictly 63 bytes for 100 bars
    for len in [100, 101, 150, 200] {
        let input = vec![15u8; len];
        let encoded = encode_waveform_5bit(&input);
        assert_eq!(
            encoded.len(),
            TELEGRAM_WAVEFORM_BYTES,
            "Capped inputs >= 100 must yield strictly 63 bytes"
        );
    }

    // Boundary bar counts
    assert_eq!(encode_waveform_5bit(&[]).len(), 0);
    assert_eq!(encode_waveform_5bit(&[10]).len(), 1);
    assert_eq!(encode_waveform_5bit(&[10; 50]).len(), 32);
    assert_eq!(encode_waveform_5bit(&[10; 99]).len(), 62);
    assert_eq!(encode_waveform_5bit(&[10; 100]).len(), 63);
}

// =========================================================================
// 2. HTTP 206 Range Streaming Invariants (TCG-N52)
// =========================================================================

#[test]
fn test_http206_suffix_ranges() {
    // RFC 7233 suffix range: bytes=-500 on varying file sizes

    // Size 1000: last 500 bytes -> [500, 999]
    let r1 = RangeStreamAdapter::parse_range_header("bytes=-500", 1000).unwrap();
    assert_eq!(r1, ByteRange { start: 500, end: 999 });
    assert_eq!(r1.length(), 500);

    // Size 500: suffix == file size -> [0, 499]
    let r2 = RangeStreamAdapter::parse_range_header("bytes=-500", 500).unwrap();
    assert_eq!(r2, ByteRange { start: 0, end: 499 });
    assert_eq!(r2.length(), 500);

    // Size 300: suffix > file size (RFC 7233 Section 2.1: names entire representation) -> [0, 299]
    let r3 = RangeStreamAdapter::parse_range_header("bytes=-500", 300).unwrap();
    assert_eq!(r3, ByteRange { start: 0, end: 299 });
    assert_eq!(r3.length(), 300);

    // Extreme suffix: bytes=-1 on size 1000 -> [999, 999]
    let r4 = RangeStreamAdapter::parse_range_header("bytes=-1", 1000).unwrap();
    assert_eq!(r4, ByteRange { start: 999, end: 999 });
    assert_eq!(r4.length(), 1);

    // Invalid suffix: bytes=-0 -> 416 Range Not Satisfiable
    let r5 = RangeStreamAdapter::parse_range_header("bytes=-0", 1000);
    assert_eq!(r5, Err(RangeError::NotSatisfiable(1000)));
}

#[test]
fn test_http206_out_of_bounds_and_inverted_ranges() {
    const TOTAL_SIZE: u64 = 1000;

    // Out of bounds: start >= total_size
    let r1 = RangeStreamAdapter::parse_range_header("bytes=2000-3000", TOTAL_SIZE);
    assert_eq!(r1, Err(RangeError::NotSatisfiable(TOTAL_SIZE)));

    let r2 = RangeStreamAdapter::parse_range_header("bytes=1000-1500", TOTAL_SIZE);
    assert_eq!(r2, Err(RangeError::NotSatisfiable(TOTAL_SIZE)));

    let r3 = RangeStreamAdapter::parse_range_header("bytes=1000-", TOTAL_SIZE);
    assert_eq!(r3, Err(RangeError::NotSatisfiable(TOTAL_SIZE)));

    // Inverted range: start > end
    let r4 = RangeStreamAdapter::parse_range_header("bytes=500-200", TOTAL_SIZE);
    assert_eq!(r4, Err(RangeError::NotSatisfiable(TOTAL_SIZE)));

    let r5 = RangeStreamAdapter::parse_range_header("bytes=999-0", TOTAL_SIZE);
    assert_eq!(r5, Err(RangeError::NotSatisfiable(TOTAL_SIZE)));

    // Zero-length file
    let r6 = RangeStreamAdapter::parse_range_header("bytes=0-100", 0);
    assert_eq!(r6, Err(RangeError::NotSatisfiable(0)));

    // Clamped upper bound: start < total_size, end >= total_size -> clamped to total_size - 1
    let r7 = RangeStreamAdapter::parse_range_header("bytes=500-2000", TOTAL_SIZE).unwrap();
    assert_eq!(r7, ByteRange { start: 500, end: 999 });
    assert_eq!(r7.length(), 500);

    // Extraction under clamped range
    let dummy_data: Vec<u8> = (0..TOTAL_SIZE).map(|i| (i % 256) as u8).collect();
    let slice = RangeStreamAdapter::extract_slice(&dummy_data, &r7).unwrap();
    assert_eq!(slice.len(), 500);
    assert_eq!(slice[0], (500 % 256) as u8);
    assert_eq!(slice[499], (999 % 256) as u8);
}

// =========================================================================
// 3. Resilient Chunked Upload Concurrency & Corruption (TCG-N51)
// =========================================================================

#[test]
fn test_chunked_upload_out_of_order_and_duplicates() {
    // 500KB file with 64KB chunk size -> 8 parts (7 full 64KB parts + 1 52KB part)
    let total_size: u64 = 500 * 1024;
    let part_size: u32 = 64 * 1024;

    let payload: Vec<u8> = (0..total_size)
        .map(|i| ((i * 31 + 7) % 256) as u8)
        .collect();

    let mut session = ChunkedUploadSession::new("test-fuzz-upload-001", total_size, part_size).unwrap();
    assert_eq!(session.total_parts, 8);

    // Ingest parts completely scrambled: [5, 2, 7, 0, 4, 1, 6, 3]
    let part_order = vec![5, 2, 7, 0, 4, 1, 6, 3];

    for &idx in &part_order {
        let start = (idx * part_size) as usize;
        let end = ((idx + 1) * part_size as u32).min(total_size as u32) as usize;
        let part_data = &payload[start..end];
        session.receive_part(idx, part_data).unwrap();
        assert!(session.is_part_received(idx));
    }

    // Now inject duplicate parts (e.g. network retransmission)
    for &idx in &[0, 2, 5, 7] {
        let start = (idx * part_size) as usize;
        let end = ((idx + 1) * part_size as u32).min(total_size as u32) as usize;
        let part_data = &payload[start..end];
        session.receive_part(idx, part_data).unwrap();
    }

    assert!(session.is_complete());
    assert!(session.missing_parts().is_empty());

    let assembled = session.assemble_payload().unwrap();
    assert_eq!(assembled.len(), total_size as usize);
    assert_eq!(assembled, payload, "Assembled payload must match original after scrambled duplicate ingestion");
}

#[test]
fn test_chunked_upload_corruption_rejection() {
    let total_size: u64 = 256 * 1024;
    let part_size: u32 = 64 * 1024;

    let payload: Vec<u8> = (0..total_size)
        .map(|i| ((i * 17) % 251) as u8)
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let expected_sha256 = format!("{:x}", hasher.finalize());

    // Scenario A: Byte corruption in payload
    let mut session = ChunkedUploadSession::new("test-corrupt-001", total_size, part_size)
        .unwrap()
        .with_expected_sha256(&expected_sha256);

    for idx in 0..session.total_parts {
        let start = (idx * part_size) as usize;
        let end = ((idx + 1) * part_size) as usize;
        let mut part_data = payload[start..end].to_vec();

        // Corrupt part 2 by flipping a bit
        if idx == 2 {
            part_data[42] ^= 0x55;
        }

        session.receive_part(idx, &part_data).unwrap();
    }

    assert!(session.is_complete());
    // Verification MUST fail due to hash mismatch
    let verified = session.finalize_and_verify().unwrap();
    assert!(!verified, "Corrupted payload MUST NOT pass SHA-256 verification");

    let assembled = session.assemble_payload().unwrap();
    assert!(!ChunkedUploadSession::verify_sha256(&assembled, &expected_sha256));

    // Scenario B: Size corruption (wrong part size)
    let mut session_size = ChunkedUploadSession::new("test-corrupt-002", total_size, part_size).unwrap();
    let truncated_part = vec![0u8; (part_size - 1) as usize];
    let err = session_size.receive_part(0, &truncated_part);
    assert!(matches!(err, Err(TelegramError::Validation(_))));

    // Scenario C: Out of bounds part index
    let err_oob = session_size.receive_part(session_size.total_parts + 5, &vec![0u8; part_size as usize]);
    assert!(matches!(err_oob, Err(TelegramError::Validation(_))));

    // Scenario D: Assembly on incomplete session
    let session_incomplete = ChunkedUploadSession::new("test-corrupt-003", total_size, part_size).unwrap();
    let err_incomplete = session_incomplete.assemble_payload();
    assert!(matches!(err_incomplete, Err(TelegramError::Validation(_))));
}

#[test]
fn test_chunked_upload_multithreaded_concurrency() {
    let total_size: u64 = 1024 * 1024; // 1MB
    let part_size: u32 = 64 * 1024;    // 16 parts

    let payload: Arc<Vec<u8>> = Arc::new(
        (0..total_size)
            .map(|i| ((i.wrapping_mul(7919) ^ 0xA5) % 256) as u8)
            .collect(),
    );

    let mut hasher = Sha256::new();
    hasher.update(&payload[..]);
    let expected_sha256 = format!("{:x}", hasher.finalize());

    let session = Arc::new(Mutex::new(
        ChunkedUploadSession::new("concurrent-upload", total_size, part_size)
            .unwrap()
            .with_expected_sha256(&expected_sha256),
    ));

    let mut handles = Vec::new();
    let total_parts = 16u32;

    // Spawn 16 concurrent threads to deliver parts with intentional race conditions and duplicates
    for t in 0..total_parts {
        let session_clone = Arc::clone(&session);
        let payload_clone = Arc::clone(&payload);

        let handle = thread::spawn(move || {
            let start = (t * part_size) as usize;
            let end = ((t + 1) * part_size) as usize;
            let part_data = &payload_clone[start..end];

            // Primary ingestion
            session_clone.lock().unwrap().receive_part(t, part_data).unwrap();

            // Interleaved duplicate ingestion
            session_clone.lock().unwrap().receive_part(t, part_data).unwrap();
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    let guard = session.lock().unwrap();
    assert!(guard.is_complete());
    assert!(guard.finalize_and_verify().unwrap());
    let assembled = guard.assemble_payload().unwrap();
    assert_eq!(&assembled[..], &payload[..]);
}
