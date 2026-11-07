use super::encoder::WAVEFORM_MAX_VALUE;

/// Decodes Telegram 5-bit packed waveform bytes back into quantized bar values (0..=31).
///
/// Upstream reference: `Telegram/SourceFiles/data/data_document.cpp` lines 1383–1414 (`documentWaveformDecode`).
pub fn decode_waveform_5bit(encoded: &[u8]) -> Vec<u8> {
    let bits_count = encoded.len() * 8;
    let values_count = bits_count / 5;
    if values_count == 0 {
        return Vec::new();
    }

    let mut result = vec![0u8; values_count];
    for i in 0..values_count.saturating_sub(1) {
        let byte_index = (i * 5) / 8;
        let bit_shift = (i * 5) % 8;
        let value = u16::from_le_bytes([encoded[byte_index], encoded[byte_index + 1]]);
        result[i] = ((value >> bit_shift) & (WAVEFORM_MAX_VALUE as u16)) as u8;
    }

    let last_index = values_count - 1;
    let last_byte_index = (last_index * 5) / 8;
    let last_bit_shift = (last_index * 5) % 8;
    let last_value = if last_byte_index + 1 < encoded.len() {
        u16::from_le_bytes([encoded[last_byte_index], encoded[last_byte_index + 1]])
    } else {
        encoded[last_byte_index] as u16
    };
    result[last_index] = ((last_value >> last_bit_shift) & (WAVEFORM_MAX_VALUE as u16)) as u8;

    result
}

/// Decodes Telegram 5-bit packed waveform bytes back into normalized amplitude peaks (0.0 ..= 1.0).
pub fn decode_waveform_to_floats(encoded: &[u8], count: usize) -> Vec<f32> {
    let bars = decode_waveform_5bit(encoded);
    let mut floats = Vec::with_capacity(count);
    for i in 0..count {
        if i < bars.len() {
            floats.push((bars[i] as f32) / 31.0);
        } else {
            floats.push(0.0);
        }
    }
    floats
}
