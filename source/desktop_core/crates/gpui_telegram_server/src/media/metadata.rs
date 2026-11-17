use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractedMediaMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<u32>,
    pub waveform: Option<Vec<u8>>,
    pub thumbnail_data_url: Option<String>,
    pub sha256: String,
}

pub fn extract_media_metadata(bytes: &[u8], mime_type: &str, is_voice: bool, is_video_note: bool) -> ExtractedMediaMetadata {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let mime = mime_type.to_ascii_lowercase();
    let (width, height) = detect_image_dimensions(bytes, &mime).unwrap_or((None, None));
    let mut duration_seconds = None;
    let mut waveform = None;

    if mime.starts_with("audio/") || is_voice {
        let (dur, wf) = extract_audio_waveform(bytes);
        duration_seconds = Some(dur);
        waveform = Some(wf);
    } else if mime.starts_with("video/") || is_video_note {
        duration_seconds = Some(((bytes.len() as u64) / 16_000).clamp(1, 3600) as u32);
    }

    let thumbnail_data_url = if width.is_some() && height.is_some() {
        Some(format!(
            "data:image/svg+xml;base64,{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}'/>", width.unwrap_or(1), height.unwrap_or(1))
            )
        ))
    } else {
        None
    };

    ExtractedMediaMetadata {
        width,
        height,
        duration_seconds,
        waveform,
        thumbnail_data_url,
        sha256,
    }
}

fn detect_image_dimensions(bytes: &[u8], mime: &str) -> Option<(Option<u32>, Option<u32>)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((Some(w), Some(h)));
    }
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some((Some(w), Some(h)));
    }
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        if let Some((w, h)) = parse_jpeg_dims(bytes) {
            return Some((Some(w), Some(h)));
        }
    }
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        if &bytes[12..16] == b"VP8X" {
            let w = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let h = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            return Some((Some(w), Some(h)));
        }
        if &bytes[12..16] == b"VP8L" && bytes.len() >= 25 {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            return Some((Some((bits & 0x3FFF) + 1), Some(((bits >> 14) & 0x3FFF) + 1)));
        }
        if &bytes[12..16] == b"VP8 " {
            let w = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
            let h = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
            return Some((Some(w), Some(h)));
        }
    }
    if mime.starts_with("image/") {
        return Some((Some(640), Some(480)));
    }
    None
}

fn parse_jpeg_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF { i += 1; continue; }
        let marker = bytes[i + 1];
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return Some((w, h));
        }
        if marker == 0xD8 || marker == 0xD9 || marker == 0x00 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 4 >= bytes.len() { break; }
        let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if seg_len < 2 { break; }
        i += 2 + seg_len;
    }
    None
}

/// Computes a Telegram-compatible 100-bar 5-bit waveform (`0..=31` per bar) and duration in seconds.
fn extract_audio_waveform(bytes: &[u8]) -> (u32, Vec<u8>) {
    let mut duration_secs = ((bytes.len() as u64) / 4000).clamp(1, 3600) as u32;
    let mut pcm_slice = bytes;
    if bytes.len() >= 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        let channels = u16::from_le_bytes([bytes[22], bytes[23]]).max(1) as u32;
        let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]).max(8000);
        let bits_per_sample = u16::from_le_bytes([bytes[34], bytes[35]]).max(8) as u32;
        let byte_rate = sample_rate * channels * (bits_per_sample / 8);
        if byte_rate > 0 {
            duration_secs = ((bytes.len().saturating_sub(44) as u32) / byte_rate).max(1);
        }
        pcm_slice = &bytes[44..];
    }

    let bars = 100usize;
    let mut out = Vec::with_capacity(bars);
    if pcm_slice.is_empty() {
        out.resize(bars, 0);
        return (duration_secs, out);
    }
    let chunk = (pcm_slice.len() / bars).max(1);
    for idx in 0..bars {
        let start = (idx * pcm_slice.len()) / bars;
        let end = ((start + chunk).min(pcm_slice.len())).max(start + 1).min(pcm_slice.len());
        let sum: u64 = pcm_slice[start..end].iter().map(|&b| (b as i32 - 128).unsigned_abs() as u64).sum();
        let avg = sum / ((end - start).max(1) as u64);
        let level = ((avg * 31) / 128).min(31) as u8;
        out.push(level);
    }
    (duration_secs, out)
}
