use gpui_telegram_media::waveform::{
    decode_waveform_5bit, decode_waveform_to_floats, encode_waveform_5bit,
    encode_waveform_from_floats, normalize_pcm_peaks, resample_and_normalize_waveform,
    TELEGRAM_WAVEFORM_BARS, TELEGRAM_WAVEFORM_BYTES,
};

#[test]
fn test_zero_silence() {
    let zeros = vec![0u8; TELEGRAM_WAVEFORM_BARS];
    let packed = encode_waveform_5bit(&zeros);
    assert_eq!(packed.len(), TELEGRAM_WAVEFORM_BYTES);
    assert!(packed.iter().all(|&b| b == 0));
    assert_eq!(decode_waveform_5bit(&packed), zeros);
}

#[test]
fn test_maximum_saturation() {
    let max_bars = vec![31u8; TELEGRAM_WAVEFORM_BARS];
    let packed = encode_waveform_5bit(&max_bars);
    assert_eq!(packed.len(), TELEGRAM_WAVEFORM_BYTES);
    assert_eq!(decode_waveform_5bit(&packed), max_bars);
}

#[test]
fn test_random_invariance() {
    for seed in 0..10u8 {
        let bars: Vec<u8> = (0..TELEGRAM_WAVEFORM_BARS)
            .map(|i| (i as u8).wrapping_mul(seed + 3) % 32)
            .collect();
        let packed = encode_waveform_5bit(&bars);
        assert_eq!(packed.len(), TELEGRAM_WAVEFORM_BYTES);
        let unpacked = decode_waveform_5bit(&packed);
        assert_eq!(unpacked, bars, "Failed for seed {}", seed);
    }
}

#[test]
fn test_boundary_lengths() {
    assert!(encode_waveform_5bit(&[]).is_empty());
    assert!(decode_waveform_5bit(&[]).is_empty());

    let one = vec![23u8];
    let packed_one = encode_waveform_5bit(&one);
    assert_eq!(packed_one.len(), 1);
    assert_eq!(decode_waveform_5bit(&packed_one), vec![23u8]);

    let fifty: Vec<u8> = (0..50).map(|i| (i % 32) as u8).collect();
    let packed_fifty = encode_waveform_5bit(&fifty);
    let decoded_fifty = decode_waveform_5bit(&packed_fifty);
    assert_eq!(&decoded_fifty[..50], &fifty[..]);

    let overflow: Vec<u8> = (0..150).map(|i| (i % 32) as u8).collect();
    let packed_overflow = encode_waveform_5bit(&overflow);
    assert_eq!(packed_overflow.len(), TELEGRAM_WAVEFORM_BYTES);
    assert_eq!(decode_waveform_5bit(&packed_overflow), &overflow[..100]);
}

#[test]
fn test_float_conversion_roundtrip() {
    let float_peaks: Vec<f32> = (0..100).map(|i| (i as f32) / 100.0).collect();
    let packed = encode_waveform_from_floats(&float_peaks);
    assert_eq!(packed.len(), 63);

    let unpacked = decode_waveform_to_floats(&packed, 100);
    assert_eq!(unpacked.len(), 100);

    for (orig, dec) in float_peaks.iter().zip(unpacked.iter()) {
        assert!((orig - dec).abs() <= 0.05);
    }
}

#[test]
fn test_pcm_peak_normalization() {
    let peaks = vec![1000u16, 2000, 3000, 4000, 5000];
    let bars = normalize_pcm_peaks(&peaks);
    assert_eq!(bars.len(), 5);
    for &b in &bars {
        assert!(b <= 31);
    }
}

#[test]
fn test_resample_and_normalize() {
    let raw = vec![128u8; 300];
    let bars = resample_and_normalize_waveform(&raw);
    assert_eq!(bars.len(), 100);
}
