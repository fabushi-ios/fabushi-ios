use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

pub const ATTACHMENT_BYTE_LIMIT: u64 = 25 * 1024 * 1024;
pub const VIDEO_BYTE_LIMIT: u64 = 200 * 1024 * 1024;
pub const BYTES_PER_MB: u64 = 1024 * 1024;
pub const BASE64_SNIFF_PREFIX_CHARS: usize = 256;

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
}

pub fn image_mime_from_path(path: &Path) -> Option<&'static str> {
    match extension(path)?.as_str() {
        ".avif" => Some("image/avif"),
        ".bmp" => Some("image/bmp"),
        ".gif" => Some("image/gif"),
        ".ico" => Some("image/x-icon"),
        ".jpeg" | ".jpg" => Some("image/jpeg"),
        ".png" => Some("image/png"),
        ".svg" => Some("image/svg+xml"),
        ".webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn servable_image_mime_from_path(path: &Path) -> Option<&'static str> {
    image_mime_from_path(path).or_else(|| match extension(path)?.as_str() {
        ".heic" => Some("image/heic"),
        ".heif" => Some("image/heif"),
        _ => None,
    })
}

pub fn video_mime_from_path(path: &Path) -> Option<&'static str> {
    match extension(path)?.as_str() {
        ".m4v" | ".mp4" => Some("video/mp4"),
        ".mov" => Some("video/quicktime"),
        ".ogv" => Some("video/ogg"),
        ".webm" => Some("video/webm"),
        _ => None,
    }
}

pub fn audio_mime_from_path(path: &Path) -> Option<&'static str> {
    match extension(path)?.as_str() {
        ".aac" => Some("audio/aac"),
        ".flac" => Some("audio/flac"),
        ".m4a" => Some("audio/mp4"),
        ".mp3" => Some("audio/mpeg"),
        ".oga" | ".ogg" | ".opus" => Some("audio/ogg"),
        ".wav" => Some("audio/wav"),
        ".weba" => Some("audio/webm"),
        _ => None,
    }
}

pub fn extension_from_image_mime(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/avif" => Some(".avif"),
        "image/bmp" => Some(".bmp"),
        "image/gif" => Some(".gif"),
        "image/jpeg" => Some(".jpg"),
        "image/png" => Some(".png"),
        "image/svg+xml" => Some(".svg"),
        "image/webp" => Some(".webp"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(".ico"),
        _ => None,
    }
}

pub fn name_looks_like_video(path: &Path) -> bool {
    video_mime_from_path(path).is_some()
}

pub fn attachment_byte_limit_for_name(path: &Path) -> u64 {
    if name_looks_like_video(path) {
        VIDEO_BYTE_LIMIT
    } else {
        ATTACHMENT_BYTE_LIMIT
    }
}

pub fn format_megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes + (BYTES_PER_MB / 2)) / BYTES_PER_MB)
}

pub fn format_attachment_too_large_notice(filename: &str) -> String {
    let video = name_looks_like_video(Path::new(filename));
    let limit = if video { VIDEO_BYTE_LIMIT } else { ATTACHMENT_BYTE_LIMIT };
    format!(
        "\"{filename}\" is too large to attach (max {}{}).",
        format_megabytes(limit),
        if video { " for video" } else { "" }
    )
}

pub const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    if buffer.len() >= 3 && buffer[..3] == [0xff, 0xd8, 0xff] {
        return if buffer.get(3) == Some(&0xf7) {
            None
        } else {
            Some("image/jpeg")
        };
    }
    if buffer.len() >= 8 && buffer[..8] == PNG_SIGNATURE {
        return if is_png(buffer) && !is_animated_png(buffer) {
            Some("image/png")
        } else {
            None
        };
    }
    if buffer.starts_with(b"GIF") {
        return Some("image/gif");
    }
    if buffer.len() >= 12 && buffer.starts_with(b"RIFF") && &buffer[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if buffer.starts_with(b"BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }
    None
}

fn read_u32_be(buffer: &[u8], offset: usize) -> Option<u32> {
    let bytes = buffer.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u32_le(buffer: &[u8], offset: usize) -> Option<u32> {
    let bytes = buffer.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u16_le(buffer: &[u8], offset: usize) -> Option<u16> {
    let bytes = buffer.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_u32_be(buffer, PNG_SIGNATURE.len()) == Some(13)
        && buffer.get(12..16) == Some(b"IHDR")
}

fn is_animated_png(buffer: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= buffer.len() {
        let chunk_length = match read_u32_be(buffer, offset) {
            Some(len) => len as usize,
            None => return false,
        };
        let chunk_type_offset = offset + 4;
        let chunk_type = match buffer.get(chunk_type_offset..chunk_type_offset + 4) {
            Some(t) => t,
            None => return false,
        };
        if chunk_type == b"acTL" {
            return true;
        }
        if chunk_type == b"IDAT" {
            return false;
        }
        let next_offset = match offset.checked_add(12).and_then(|o| o.checked_add(chunk_length)) {
            Some(next) => next,
            None => return false,
        };
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }
    let declared_file_size = match read_u32_le(buffer, 2) {
        Some(s) => s as usize,
        None => return false,
    };
    let pixel_data_offset = match read_u32_le(buffer, 10) {
        Some(s) => s as usize,
        None => return false,
    };
    let dib_header_size = match read_u32_le(buffer, 14) {
        Some(s) => s as usize,
        None => return false,
    };
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let (color_planes, bits_per_pixel) = if dib_header_size == 12 {
        let planes = match read_u16_le(buffer, 22) {
            Some(p) => p,
            None => return false,
        };
        let bpp = match read_u16_le(buffer, 24) {
            Some(b) => b,
            None => return false,
        };
        (planes, bpp)
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        let planes = match read_u16_le(buffer, 26) {
            Some(p) => p,
            None => return false,
        };
        let bpp = match read_u16_le(buffer, 28) {
            Some(b) => b,
            None => return false,
        };
        (planes, bpp)
    } else {
        return false;
    };

    color_planes == 1 && matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32)
}

pub fn sniff_mime_from_buffer(bytes: &[u8]) -> Option<&'static str> {
    if let Some(image_mime) = detect_supported_image_mime_type(bytes) {
        return Some(image_mime);
    }
    if bytes.len() >= 8 && bytes[..8] == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes[..3] == [0xff, 0xd8, 0xff] {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 5 && &bytes[..5] == b"%PDF-" {
        return Some("application/pdf");
    }
    None
}

fn base64_char_value(code: u8) -> Option<u8> {
    match code {
        b'A'..=b'Z' => Some(code - b'A'),
        b'a'..=b'z' => Some(code - b'a' + 26),
        b'0'..=b'9' => Some(code - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub struct CanonicalBase64Facts {
    pub cleaned_base64: String,
    pub decoded_byte_len: usize,
    pub canonical_pad_bits: bool,
}

pub fn inspect_canonical_base64(raw: &str) -> Option<CanonicalBase64Facts> {
    let mut cleaned = Vec::with_capacity(raw.len());
    let mut padding = 0usize;
    let mut last_data_value = 0u8;

    for &byte in raw.as_bytes() {
        if byte <= 0x20 {
            // Canonical base64 ignores ASCII whitespace
            continue;
        }
        if byte == b'=' {
            padding += 1;
            if padding > 2 {
                return None;
            }
        } else {
            if padding > 0 {
                // Characters after padding are forbidden
                return None;
            }
            let value = base64_char_value(byte)?;
            last_data_value = value;
        }
        cleaned.push(byte);
    }

    let cleaned_len = cleaned.len();
    if cleaned_len == 0 {
        return None;
    }
    let remainder = cleaned_len % 4;
    if remainder != 0 && (padding > 0 || remainder == 1) {
        return None;
    }

    let effective_padding = if remainder == 0 { padding } else { 4 - remainder };
    let pad_bit_mask = match effective_padding {
        2 => 0x0f,
        1 => 0x03,
        _ => 0x00,
    };
    let canonical_pad_bits = (last_data_value & pad_bit_mask) == 0;
    if !canonical_pad_bits {
        return None;
    }

    if remainder != 0 {
        for _ in 0..(4 - remainder) {
            cleaned.push(b'=');
        }
    }

    let decoded_byte_len = (cleaned.len() / 4) * 3 - padding;
    let cleaned_base64 = String::from_utf8(cleaned).ok()?;
    Some(CanonicalBase64Facts {
        cleaned_base64,
        decoded_byte_len,
        canonical_pad_bits: true,
    })
}

#[derive(Debug, Clone, Default)]
pub struct MimeSniffHints<'a> {
    pub header_mime: Option<&'a str>,
    pub file_path: Option<&'a Path>,
    pub additional_hints: &'a [&'a str],
}

pub fn sniff_mime_from_base64_with_hints(
    base64_str: &str,
    hints: &MimeSniffHints,
) -> Option<String> {
    // 1. Inspect whole payload for canonical base64 conformance and canonical pad bits
    let facts = inspect_canonical_base64(base64_str)?;

    // 2. Bound decoding to BASE64_SNIFF_PREFIX_CHARS
    let take = facts.cleaned_base64.len().min(BASE64_SNIFF_PREFIX_CHARS);
    let slice_len = take - (take % 4);
    if slice_len < 8 {
        return None;
    }
    let slice = &facts.cleaned_base64[..slice_len];
    let decoded = STANDARD.decode(slice).ok()?;

    // 3. First check binary magic bytes (PNG, JPEG, GIF, WEBP, PDF)
    if let Some(sniffed) = sniff_mime_from_buffer(&decoded) {
        return Some(sniffed.to_string());
    }

    // 4. File extension hint takes precedence over header/generic metadata
    // (matches upstream detectMime priority: inferred = (sniffed ?? extMime) ?? headerMime)
    if let Some(path) = hints.file_path {
        if let Some(path_mime) = servable_image_mime_from_path(path)
            .or_else(|| video_mime_from_path(path))
            .or_else(|| audio_mime_from_path(path))
            .or_else(|| match extension(path).as_deref() {
                Some(".pdf") => Some("application/pdf"),
                Some(".txt") => Some("text/plain"),
                Some(".json") => Some("application/json"),
                _ => None,
            })
        {
            return Some(path_mime.to_string());
        }
    }

    // 5. Fallback to header_mime and additional hints
    if let Some(header) = hints.header_mime {
        let trimmed = header.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    for hint in hints.additional_hints {
        let trimmed = hint.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn sniff_mime_from_base64(base64_str: &str) -> Option<String> {
    sniff_mime_from_base64_with_hints(base64_str, &MimeSniffHints::default())
}
