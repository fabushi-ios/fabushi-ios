pub const TELEGRAM_WAVEFORM_BARS: usize = 100;
pub const TELEGRAM_WAVEFORM_BYTES: usize = 63;
pub const WAVEFORM_MAX_VALUE: u8 = 31;
pub const PEAK_AVERAGE_RATIO: f64 = 1.8;
pub const MINIMAL_PEAK_FLOOR: i32 = 2500;

/// Encodes an array of quantized bar values (0..=31) into Telegram 5-bit packed bytes.
///
/// Upstream reference: `Telegram/SourceFiles/data/data_document.cpp` lines 1416–1434 (`documentWaveformEncode5bit`).
/// For 100 bars, the packed output is exactly 63 bytes.
pub fn encode_waveform_5bit(bars: &[u8]) -> Vec<u8> {
    let count = bars.len().min(TELEGRAM_WAVEFORM_BARS);
    if count == 0 {
        return Vec::new();
    }

    let bits_count = count * 5;
    let bytes_count = (bits_count + 7) / 8;
    // Allocate bytes_count + 1 to safely write 16-bit unaligned little-endian windows
    let mut result = vec![0u8; bytes_count + 1];

    for (i, &bar) in bars.iter().take(count).enumerate() {
        let byte_index = (i * 5) / 8;
        let bit_shift = (i * 5) % 8;
        let value = ((bar & WAVEFORM_MAX_VALUE) as u16) << bit_shift;
        let previous = u16::from_le_bytes([result[byte_index], result[byte_index + 1]]);
        let combined = previous | value;
        let le = combined.to_le_bytes();
        result[byte_index] = le[0];
        result[byte_index + 1] = le[1];
    }

    result.truncate(bytes_count);
    result
}

/// Normalizes raw PCM peaks into 0..31 bars using dynamic ceiling compression.
///
/// Upstream reference: `Telegram/SourceFiles/media/audio/media_audio_capture.cpp` lines 68–76 (`CollectWaveform`).
pub fn normalize_pcm_peaks(peaks: &[u16]) -> Vec<u8> {
    if peaks.is_empty() {
        return Vec::new();
    }

    let count = peaks.len().min(TELEGRAM_WAVEFORM_BARS);
    let selected = &peaks[..count];
    let sum: i64 = selected.iter().map(|&p| p as i64).sum();
    let avg = (sum as f64) * PEAK_AVERAGE_RATIO / (count as f64);
    let dynamic_ceiling = (avg.round() as i32).max(MINIMAL_PEAK_FLOOR) as u32;

    selected
        .iter()
        .map(|&p| {
            let clamped = (p as u32).min(dynamic_ceiling);
            ((clamped * 31) / dynamic_ceiling).min(31) as u8
        })
        .collect()
}

/// Encodes normalized amplitude float peaks (0.0 ..= 1.0) into Telegram 5-bit packed bytes.
pub fn encode_waveform_from_floats(peaks: &[f32]) -> Vec<u8> {
    let bars: Vec<u8> = peaks
        .iter()
        .take(TELEGRAM_WAVEFORM_BARS)
        .map(|&p| (p.clamp(0.0, 1.0) * 31.0).round() as u8)
        .collect();
    encode_waveform_5bit(&bars)
}
