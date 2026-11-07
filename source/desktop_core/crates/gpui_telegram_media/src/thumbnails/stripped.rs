use gpui_telegram_core::error::{Result, TelegramError};

pub const TELEGRAM_STRIPPED_PREFIX: u8 = 0x01;

/// Decodes Telegram inline stripped photo preview data.
///
/// Upstream format:
/// Byte 0: 0x01 format marker
/// Byte 1: width (pixels)
/// Byte 2: height (pixels)
/// Bytes 3..: JPEG scan payload
pub fn parse_stripped_dimensions(data: &[u8]) -> Result<(u32, u32)> {
    if data.len() < 3 {
        return Err(TelegramError::Validation(
            "Stripped photo data too short (must be >= 3 bytes)".into(),
        ));
    }
    if data[0] != TELEGRAM_STRIPPED_PREFIX {
        return Err(TelegramError::Validation(format!(
            "Invalid stripped photo marker: expected 0x01, found 0x{:02x}",
            data[0]
        )));
    }
    let w = data[1] as u32;
    let h = data[2] as u32;
    Ok((w.max(1), h.max(1)))
}

/// Envelopes a JPEG scan payload into a Telegram inline stripped format.
pub fn envelope_stripped_photo(width: u8, height: u8, scan_data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + scan_data.len());
    out.push(TELEGRAM_STRIPPED_PREFIX);
    out.push(width);
    out.push(height);
    out.extend_from_slice(scan_data);
    out
}
