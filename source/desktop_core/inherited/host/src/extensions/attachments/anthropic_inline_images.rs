//! Normalization of Anthropic inline content blocks with image format conversion.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/anthropic-inline-images.ts`
//! - `src/media/anthropic-inline-images.test.ts`

use serde::{Deserialize, Serialize};
use crate::extensions::attachments::image_ops::{
    convert_bmp_to_png, read_image_probe_from_header,
};

pub const ANTHROPIC_SUPPORTED_IMAGE_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
];

pub const ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES: usize = 10 * 1024 * 1024;
pub const FILE_TYPE_SNIFF_MAX_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

pub fn is_anthropic_supported_mime(mime: &str) -> bool {
    let lower = mime.trim().to_ascii_lowercase();
    ANTHROPIC_SUPPORTED_IMAGE_MIMES.contains(&lower.as_str())
}

/// Estimates decoded byte count from raw base64 string without full buffer allocation.
pub fn estimate_base64_decoded_bytes(b64: &str) -> usize {
    let compact: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if compact.is_empty() {
        return 0;
    }
    let padding = if compact.ends_with("==") {
        2
    } else if compact.ends_with('=') {
        1
    } else {
        0
    };
    (compact.len() * 3) / 4 - padding
}

/// Normalizes a single Anthropic image block, converting unsupported formats (such as BMP) to PNG.
pub fn normalize_anthropic_inline_image_block(
    data: &str,
    declared_mime: &str,
) -> Result<(String, String), String> {
    use base64::Engine as _;
    let trimmed = data.trim();
    let estimated = estimate_base64_decoded_bytes(trimmed);
    if estimated > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
        return Err("Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
    }

    let compact: String = trimmed.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .map_err(|e| format!("Invalid base64 payload: {e}"))?;

    if decoded.len() > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
        return Err("Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
    }

    let probe = read_image_probe_from_header(&decoded);
    let detected_format = probe.as_ref().map(|p| p.format.as_str());

    let norm_declared = declared_mime.trim().to_ascii_lowercase();

    if norm_declared == "image/bmp" || detected_format == Some("bmp") {
        let png_bytes = convert_bmp_to_png(&decoded)?;
        if png_bytes.len() > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
            return Err("Normalized Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
        }
        let b64_out = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        Ok((b64_out, "image/png".to_string()))
    } else if let Some(fmt) = detected_format {
        let mime = match fmt {
            "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => {
                // If detected is unsupported (e.g. tiff), but we have a supported declared fallback
                if is_anthropic_supported_mime(&norm_declared) {
                    norm_declared.as_str()
                } else {
                    "image/jpeg"
                }
            }
        };
        Ok((trimmed.to_string(), mime.to_string()))
    } else if is_anthropic_supported_mime(&norm_declared) {
        Ok((trimmed.to_string(), norm_declared))
    } else {
        Ok((trimmed.to_string(), "image/jpeg".to_string()))
    }
}

/// Normalizes an array of Anthropic content blocks, validating safety caps before processing.
pub fn normalize_anthropic_inline_content_blocks(
    blocks: &[AnthropicContentBlock],
) -> Result<Vec<AnthropicContentBlock>, String> {
    for b in blocks {
        if let AnthropicContentBlock::Image { data, .. } = b {
            if estimate_base64_decoded_bytes(data) > ANTHROPIC_INLINE_IMAGE_DECODE_SAFETY_BYTES {
                return Err("Anthropic inline image exceeds the 10 MB decoded safety limit.".to_string());
            }
        }
    }

    let mut out = Vec::with_capacity(blocks.len());
    for b in blocks {
        match b {
            AnthropicContentBlock::Text { text } => {
                out.push(AnthropicContentBlock::Text { text: text.clone() });
            }
            AnthropicContentBlock::Image { data, mime_type } => {
                let (norm_data, norm_mime) =
                    normalize_anthropic_inline_image_block(data, mime_type)?;
                out.push(AnthropicContentBlock::Image {
                    data: norm_data,
                    mime_type: norm_mime,
                });
            }
        }
    }
    Ok(out)
}
