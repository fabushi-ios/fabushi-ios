//! Image operations, display metadata resolution, and attachment preparation.
//!
//! Maps upstream OpenClaw media modules:
//! - `src/media/image-ops.ts`
//! - `src/media/image-processor.ts`
//! - `src/media/image-processor.worker.ts`
//! - `src/media/attachment-processor.runtime.ts`
//! - `src/media/attachment-processor.worker.ts`
//! - `src/media/image-ops.helpers.test.ts`
//! - `src/media/image-ops.rastermill-adapter.test.ts`
//! - `src/media/image-ops.worker.test.ts`
//! - `src/media/image-processor.pixel-limits.test.ts`
//! - `src/media/photon.runtime.test.ts`

use std::path::Path;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

pub const MAX_IMAGE_INPUT_PIXELS: usize = 25_000_000;
pub const DOWNSCALE_IMAGE_INPUT_PIXELS: usize = 40_000_000;
pub const ATTACHMENT_OFFLOAD_THRESHOLD_BYTES: usize = 2_000_000;
pub const MAX_IMAGE_BYTES: usize = 20_000_000;
pub const ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES: usize = 10 * 1024 * 1024;

pub const IMAGE_REDUCE_QUALITY_STEPS: &[u32] = &[85, 75, 65, 55, 45, 35];
pub const WITHIN_BYTES_QUALITY_STEPS: &[u32] = &[92, 85, 78, 70, 62, 52, 42];
pub const WITHIN_BYTES_SIDE_SCALES: &[f64] = &[1.0, 0.85, 0.7, 0.5, 0.35, 0.25];

/// Display dimensions of an image, taking orientation swaps into account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
}

/// Detailed probe result from header inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageProbe {
    pub width: u32,
    pub height: u32,
    pub format: String,
    #[serde(rename = "hasAlpha")]
    pub has_alpha: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<u32>,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInput {
    pub base64: String,
    pub label: String,
    pub mime: String,
    #[serde(rename = "imageStorage", skip_serializing_if = "Option::is_none")]
    pub image_storage: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAttachment {
    #[serde(rename = "sizeBytes")]
    pub size_bytes: usize,
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Builds a descending, de-duplicated max-side search grid for iterative image resizing.
pub fn build_image_resize_side_grid(max_side: u32, side_start: u32) -> Vec<u32> {
    let candidates = [side_start, 1800, 1600, 1400, 1200, 1000, 800];
    let mut grid: Vec<u32> = candidates
        .iter()
        .map(|&val| std::cmp::min(max_side, val))
        .filter(|&val| val > 0)
        .collect();
    grid.sort_unstable_by(|a, b| b.cmp(a));
    grid.dedup();
    grid
}

/// Detects animated WebP before a single-frame image transform can discard its frames.
///
/// Follows RFC 9649 §2.7 VP8X animation flag (bit 1 of flags byte at offset 20).
pub fn is_animated_webp_buffer(buffer: &[u8]) -> bool {
    if buffer.len() < 30 {
        return false;
    }
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WEBP" || &buffer[12..16] != b"VP8X" {
        return false;
    }
    let vp8x_chunk_len = u32::from_le_bytes(buffer[16..20].try_into().unwrap()) as usize;
    if vp8x_chunk_len < 10 || vp8x_chunk_len > buffer.len() - 20 {
        return false;
    }
    (buffer[20] & 0x02) != 0
}

/// Confirms PNG is still without treating a truncated or capped scan as proof.
///
/// Scans up to 512 chunks from offset 8. Rejects if "acTL" or "IEND" is seen before "IDAT".
pub fn is_still_png_buffer(buffer: &[u8]) -> bool {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if buffer.len() < 8 || &buffer[0..8] != PNG_SIG {
        return false;
    }
    let mut offset = 8usize;
    let mut chunks = 0;
    while chunks < 512 && offset + 12 <= buffer.len() {
        let chunk_len = u32::from_be_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset + 12 + chunk_len;
        if end > buffer.len() {
            return false;
        }
        let chunk_type = &buffer[offset + 4..offset + 8];
        if chunk_type == b"acTL" || chunk_type == b"IEND" {
            return false;
        }
        if chunk_type == b"IDAT" {
            return true;
        }
        offset = end;
        chunks += 1;
    }
    false
}

/// Helper to parse EXIF orientation from JPEG App1 marker.
fn parse_jpeg_exif_orientation(buffer: &[u8]) -> Option<u32> {
    if buffer.len() < 4 || buffer[0] != 0xFF || buffer[1] != 0xD8 {
        return None;
    }
    let mut offset = 2usize;
    while offset + 4 <= buffer.len() {
        if buffer[offset] != 0xFF {
            offset += 1;
            continue;
        }
        let marker = buffer[offset + 1];
        if marker == 0xFF {
            offset += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0xD9 {
            // SOI or EOI
            offset += 2;
            continue;
        }
        let len = u16::from_be_bytes(buffer[offset + 2..offset + 4].try_into().ok()?) as usize;
        if len < 2 || offset + 2 + len > buffer.len() {
            break;
        }
        if marker == 0xE1 {
            // App1 marker
            let app1_payload = &buffer[offset + 4..offset + 2 + len];
            if app1_payload.len() >= 14 && &app1_payload[0..6] == b"Exif\0\0" {
                let tiff = &app1_payload[6..];
                if tiff.len() >= 8 {
                    let is_le = match &tiff[0..2] {
                        b"II" => true,
                        b"MM" => false,
                        _ => return None,
                    };
                    let magic = if is_le {
                        u16::from_le_bytes(tiff[2..4].try_into().ok()?)
                    } else {
                        u16::from_be_bytes(tiff[2..4].try_into().ok()?)
                    };
                    if magic == 42 {
                        let ifd_offset = if is_le {
                            u32::from_le_bytes(tiff[4..8].try_into().ok()?) as usize
                        } else {
                            u32::from_be_bytes(tiff[4..8].try_into().ok()?) as usize
                        };
                        if ifd_offset + 2 <= tiff.len() {
                            let entries_count = if is_le {
                                u16::from_le_bytes(tiff[ifd_offset..ifd_offset + 2].try_into().ok()?) as usize
                            } else {
                                u16::from_be_bytes(tiff[ifd_offset..ifd_offset + 2].try_into().ok()?) as usize
                            };
                            let mut entry_pos = ifd_offset + 2;
                            for _ in 0..entries_count {
                                if entry_pos + 12 > tiff.len() {
                                    break;
                                }
                                let tag = if is_le {
                                    u16::from_le_bytes(tiff[entry_pos..entry_pos + 2].try_into().ok()?)
                                } else {
                                    u16::from_be_bytes(tiff[entry_pos..entry_pos + 2].try_into().ok()?)
                                };
                                if tag == 0x0112 {
                                    // Orientation tag
                                    let val = if is_le {
                                        u16::from_le_bytes(tiff[entry_pos + 8..entry_pos + 10].try_into().ok()?)
                                    } else {
                                        u16::from_be_bytes(tiff[entry_pos + 8..entry_pos + 10].try_into().ok()?)
                                    };
                                    return Some(val as u32);
                                }
                                entry_pos += 12;
                            }
                        }
                    }
                }
            }
        }
        if marker == 0xDA {
            // SOS marker; header reading ends
            break;
        }
        offset += 2 + len;
    }
    None
}

/// Reads image probe metadata from raw header bytes without full decode.
pub fn read_image_probe_from_header(buffer: &[u8]) -> Option<ImageProbe> {
    if buffer.is_empty() {
        return None;
    }

    // 1. JPEG
    if buffer.len() >= 4 && buffer[0] == 0xFF && buffer[1] == 0xD8 {
        let orientation = parse_jpeg_exif_orientation(buffer);
        let mut offset = 2usize;
        while offset + 4 <= buffer.len() {
            if buffer[offset] != 0xFF {
                offset += 1;
                continue;
            }
            let marker = buffer[offset + 1];
            if marker == 0xFF {
                offset += 1;
                continue;
            }
            if marker == 0xDA || marker == 0xD9 {
                break;
            }
            let len = u16::from_be_bytes(buffer[offset + 2..offset + 4].try_into().ok()?) as usize;
            if len < 2 || offset + 2 + len > buffer.len() {
                break;
            }
            // SOF0 (0xC0), SOF1 (0xC1), SOF2 (0xC2), SOF3 (0xC3), SOF5..SOF7, SOF9..SOF11, SOF13..SOF15
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                if offset + 9 <= buffer.len() {
                    let height = u16::from_be_bytes(buffer[offset + 5..offset + 7].try_into().ok()?) as u32;
                    let width = u16::from_be_bytes(buffer[offset + 7..offset + 9].try_into().ok()?) as u32;
                    if width > 0 && height > 0 {
                        return Some(ImageProbe {
                            width,
                            height,
                            format: "jpeg".to_string(),
                            has_alpha: false,
                            orientation,
                            bytes: buffer.len(),
                        });
                    }
                }
            }
            offset += 2 + len;
        }
        return None;
    }

    // 2. PNG
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if buffer.len() >= 24 && &buffer[0..8] == PNG_SIG && &buffer[12..16] == b"IHDR" {
        let width = u32::from_be_bytes(buffer[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(buffer[20..24].try_into().ok()?);
        let color_type = buffer[25];
        let has_alpha = color_type == 4 || color_type == 6;
        if width > 0 && height > 0 {
            return Some(ImageProbe {
                width,
                height,
                format: "png".to_string(),
                has_alpha,
                orientation: None,
                bytes: buffer.len(),
            });
        }
    }

    // 3. BMP
    if buffer.len() >= 26 && buffer[0] == 0x42 && buffer[1] == 0x4D {
        let width = i32::from_le_bytes(buffer[18..22].try_into().ok()?);
        let height = i32::from_le_bytes(buffer[22..26].try_into().ok()?);
        let bpp = if buffer.len() >= 30 {
            u16::from_le_bytes(buffer[28..30].try_into().unwrap_or([0, 0]))
        } else {
            24
        };
        let abs_height = height.unsigned_abs();
        if width > 0 && abs_height > 0 {
            return Some(ImageProbe {
                width: width as u32,
                height: abs_height,
                format: "bmp".to_string(),
                has_alpha: bpp == 32,
                orientation: None,
                bytes: buffer.len(),
            });
        }
    }

    // 4. GIF
    if buffer.len() >= 10 && (buffer.starts_with(b"GIF87a") || buffer.starts_with(b"GIF89a")) {
        let width = u16::from_le_bytes(buffer[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(buffer[8..10].try_into().ok()?) as u32;
        if width > 0 && height > 0 {
            return Some(ImageProbe {
                width,
                height,
                format: "gif".to_string(),
                has_alpha: true,
                orientation: None,
                bytes: buffer.len(),
            });
        }
    }

    // 5. WebP
    if buffer.len() >= 30 && &buffer[0..4] == b"RIFF" && &buffer[8..12] == b"WEBP" {
        match &buffer[12..16] {
            b"VP8 " => {
                let width = (u16::from_le_bytes(buffer[26..28].try_into().ok()?) & 0x3FFF) as u32;
                let height = (u16::from_le_bytes(buffer[28..30].try_into().ok()?) & 0x3FFF) as u32;
                if width > 0 && height > 0 {
                    return Some(ImageProbe {
                        width,
                        height,
                        format: "webp".to_string(),
                        has_alpha: false,
                        orientation: None,
                        bytes: buffer.len(),
                    });
                }
            }
            b"VP8L" => {
                let p = u32::from_le_bytes(buffer[21..25].try_into().ok()?);
                let width = (p & 0x3FFF) + 1;
                let height = ((p >> 14) & 0x3FFF) + 1;
                return Some(ImageProbe {
                    width,
                    height,
                    format: "webp".to_string(),
                    has_alpha: true,
                    orientation: None,
                    bytes: buffer.len(),
                });
            }
            b"VP8X" => {
                let w = u32::from(buffer[24]) | (u32::from(buffer[25]) << 8) | (u32::from(buffer[26]) << 16);
                let h = u32::from(buffer[27]) | (u32::from(buffer[28]) << 8) | (u32::from(buffer[29]) << 16);
                let has_alpha = (buffer[20] & 0x10) != 0;
                return Some(ImageProbe {
                    width: w + 1,
                    height: h + 1,
                    format: "webp".to_string(),
                    has_alpha,
                    orientation: None,
                    bytes: buffer.len(),
                });
            }
            _ => {}
        }
    }

    None
}

/// Reads display dimensions from image header bytes, swapping axes for EXIF orientations 5-8.
pub fn read_image_metadata_from_header(buffer: &[u8]) -> Option<ImageMetadata> {
    let probe = read_image_probe_from_header(buffer)?;
    if let Some(orient) = probe.orientation {
        if (5..=8).contains(&orient) {
            return Some(ImageMetadata {
                width: probe.height,
                height: probe.width,
            });
        }
    }
    Some(ImageMetadata {
        width: probe.width,
        height: probe.height,
    })
}

/// Checks if a MIME string represents a generic container (zip or octet-stream).
pub fn is_generic_container_mime(mime: Option<&str>) -> bool {
    matches!(mime, Some("application/zip") | Some("application/octet-stream"))
}

/// BMP to PNG conversion with pixel limit validation.
pub fn convert_bmp_to_png(buffer: &[u8]) -> Result<Vec<u8>, String> {
    if let Some(probe) = read_image_probe_from_header(buffer) {
        if probe.format == "bmp" {
            let pixels = (probe.width as u64) * (probe.height as u64);
            if pixels > MAX_IMAGE_INPUT_PIXELS as u64 {
                return Err(format!(
                    "BMP dimensions {}x{} exceeds max pixels cap {}",
                    probe.width, probe.height, MAX_IMAGE_INPUT_PIXELS
                ));
            }
        }
    }
    crate::extensions::attachments::media_qr_png::convert_bmp_to_png(buffer)
}

/// Normalizes inline image blocks for model consumption, converting unsupported BMP to PNG.
pub fn normalize_anthropic_inline_image(
    data_base64: &str,
    declared_mime: &str,
) -> Result<(String, String), String> {
    let compact: String = data_base64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = BASE64_STANDARD
        .decode(&compact)
        .map_err(|e| format!("Invalid base64 in inline image: {e}"))?;

    if bytes.len() > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
        return Err("Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
    }

    let probe = read_image_probe_from_header(&bytes);
    let detected_format = probe.as_ref().map(|p| p.format.as_str());

    if declared_mime == "image/bmp" || detected_format == Some("bmp") {
        let png_bytes = convert_bmp_to_png(&bytes)?;
        if png_bytes.len() > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
            return Err("Normalized Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
        }
        let b64 = BASE64_STANDARD.encode(&png_bytes);
        Ok((b64, "image/png".to_string()))
    } else {
        let mime = match detected_format {
            Some("jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => declared_mime,
        };
        Ok((compact, mime.to_string()))
    }
}

/// Prepares an attachment by inspecting base64, sniffing MIME type, and extracting image dimensions.
pub fn prepare_attachment(input: &AttachmentInput) -> Result<PreparedAttachment, String> {
    let compact: String = input.base64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let decoded = BASE64_STANDARD
        .decode(&compact)
        .map_err(|_| format!("attachment {}: invalid base64 content", input.label))?;

    let size_bytes = decoded.len();
    let mut mime = input.mime.trim().to_lowercase();
    if mime.is_empty() || is_generic_container_mime(Some(&mime)) {
        if let Some(probe) = read_image_probe_from_header(&decoded) {
            mime = format!("image/{}", probe.format);
        } else if let Some(ext) = Path::new(&input.label).extension().and_then(|s| s.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "png" => mime = "image/png".to_string(),
                "jpg" | "jpeg" => mime = "image/jpeg".to_string(),
                "gif" => mime = "image/gif".to_string(),
                "webp" => mime = "image/webp".to_string(),
                "bmp" => mime = "image/bmp".to_string(),
                "txt" => mime = "text/plain".to_string(),
                "json" => mime = "application/json".to_string(),
                _ => mime = "application/octet-stream".to_string(),
            }
        } else {
            mime = "application/octet-stream".to_string();
        }
    }

    let mut dimensions = None;
    if mime.starts_with("image/") && size_bytes <= MAX_IMAGE_BYTES {
        dimensions = read_image_metadata_from_header(&decoded);
    }

    let buffer = if input.image_storage.as_deref() == Some("inline") {
        None
    } else {
        Some(decoded)
    };

    Ok(PreparedAttachment {
        size_bytes,
        mime,
        buffer,
        width: dimensions.map(|d| d.width),
        height: dimensions.map(|d| d.height),
    })
}
