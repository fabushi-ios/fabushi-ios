//! Web media delivery, bounded URL/path loading, and image optimization pipeline.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/web-media.ts`
//! - `src/media/web-media.test.ts`
//! - `src/media/web-media.animated-webp.test.ts`
//! - `src/media/web-media.file-url.windows.test.ts`

use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extensions::attachments::image_ops::{
    build_image_resize_side_grid, is_animated_webp_buffer, read_image_metadata_from_header,
    read_image_probe_from_header, ImageMetadata, MAX_IMAGE_BYTES,
};
use crate::extensions::attachments::media_file_context::ImageOptimizationLimitError;
use crate::extensions::attachments::media_reference::read_outbound_media_file_bounded;
use crate::extensions::attachments::media_store::extract_original_filename;
use crate::extensions::attachments::media_store_shared::format_media_size;

pub const IMAGE_OPTIMIZE_HEADROOM_FACTOR: usize = 4;
pub const DEFAULT_VISION_MAX_SIDE: u32 = 2048;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMediaResult {
    pub buffer: Vec<u8>,
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(rename = "fileName", skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(rename = "trustedGeneratedHtmlSource", skip_serializing_if = "Option::is_none")]
    pub trusted_generated_html_source: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageCompressionModelPolicy {
    #[serde(rename = "maxBytes", skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(rename = "maxPixels", skip_serializing_if = "Option::is_none")]
    pub max_pixels: Option<usize>,
    #[serde(rename = "maxSidePx", skip_serializing_if = "Option::is_none")]
    pub max_side_px: Option<u32>,
    #[serde(rename = "preferredSidePx", skip_serializing_if = "Option::is_none")]
    pub preferred_side_px: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageCompressionPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ImageCompressionModelPolicy>>,
    #[serde(rename = "imageCount", skip_serializing_if = "Option::is_none")]
    pub image_count: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct WebMediaOptions {
    pub max_bytes: Option<usize>,
    pub optimize_images: bool,
    pub image_compression: Option<ImageCompressionPolicy>,
    pub workspace_dir: Option<PathBuf>,
    pub local_roots: Option<Vec<PathBuf>>,
    pub sandbox_validated: bool,
    pub host_read_capability: bool,
    pub html_publication: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WebMediaError {
    #[error("Local media access error [{code}]: {message}")]
    LocalAccess { code: String, message: String },
    #[error("Image optimization limit error: {0}")]
    OptimizationLimit(#[from] ImageOptimizationLimitError),
    #[error("Media exceeds {0} limit")]
    TooLarge(String),
    #[error("Image dimensions exceed model image limits: {0}")]
    DimensionsExceeded(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

/// Strips legacy 'MEDIA:' directive prefix while preserving 'media://' scheme.
pub fn strip_legacy_media_directive_prefix(media_url: &str) -> String {
    let trimmed = media_url.trim();
    if trimmed.to_ascii_lowercase().starts_with("media://") {
        return trimmed.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("MEDIA:") {
        rest.trim().to_string()
    } else if let Some(rest) = trimmed.strip_prefix("media:") {
        rest.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Rejects Windows UNC/network paths such as \\attacker\share or //attacker/share.
pub fn assert_no_windows_network_path(path_str: &str, label: &str) -> Result<(), WebMediaError> {
    if path_str.starts_with(r"\\") || path_str.starts_with("//") {
        return Err(WebMediaError::LocalAccess {
            code: "network-path-not-allowed".to_string(),
            message: format!("{label}: Windows network share paths are not allowed ({path_str})"),
        });
    }
    Ok(())
}

/// Computes effective image byte cap based on base cap and policy models.
pub fn effective_image_bytes_cap(
    base_cap: Option<usize>,
    policy: Option<&ImageCompressionPolicy>,
) -> Option<usize> {
    let policy_cap = policy
        .and_then(|p| p.models.as_ref())
        .and_then(|models| {
            models
                .iter()
                .filter_map(|m| m.max_bytes)
                .min()
        });

    match (base_cap, policy_cap) {
        (None, p) => p,
        (b, None) => b,
        (Some(b), Some(p)) => Some(std::cmp::min(b, p)),
    }
}

/// Converts filename to target extension based on MIME type.
pub fn to_image_file_name(file_name: Option<&str>, mime_type: &str) -> Option<String> {
    let name = file_name?;
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = match mime_type {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => return Some(name.to_string()),
    };
    Some(format!("{stem}{ext}"))
}

/// Optimizes image bytes for web media delivery, preserving animated WebP/GIF when possible.
pub fn optimize_image_buffer_for_web_media(
    buffer: Vec<u8>,
    content_type: Option<&str>,
    file_name: Option<&str>,
    max_bytes: Option<usize>,
    policy: Option<&ImageCompressionPolicy>,
) -> Result<WebMediaResult, WebMediaError> {
    let base_cap = max_bytes.unwrap_or(MAX_IMAGE_BYTES);
    let cap = effective_image_bytes_cap(Some(base_cap), policy).unwrap_or(base_cap);

    let is_animated = is_animated_webp_buffer(&buffer);
    let mime = if is_animated {
        "image/webp"
    } else {
        content_type.unwrap_or("image/jpeg")
    };

    // If animated WebP or GIF, preserve animation without single-frame re-encode
    if is_animated || mime == "image/gif" {
        if buffer.len() > cap {
            let label = if is_animated { "Animated WebP" } else { "GIF" };
            return Err(WebMediaError::OptimizationLimit(ImageOptimizationLimitError {
                message: format!("{label} exceeds {} limit (got {})", format_media_size(cap as u64), format_media_size(buffer.len() as u64)),
                max_bytes: cap,
            }));
        }

        // Validate dimension caps from model policy if present
        if let Some(models) = policy.and_then(|p| p.models.as_ref()) {
            if let Some(meta) = read_image_metadata_from_header(&buffer) {
                let pixels = (meta.width as usize) * (meta.height as usize);
                for m in models {
                    if let Some(max_px) = m.max_pixels {
                        if pixels > max_px {
                            return Err(WebMediaError::DimensionsExceeded(format!(
                                "dimensions {}x{} exceeds max pixels {}",
                                meta.width, meta.height, max_px
                            )));
                        }
                    }
                    if let Some(max_side) = m.max_side_px {
                        if std::cmp::max(meta.width, meta.height) > max_side {
                            return Err(WebMediaError::DimensionsExceeded(format!(
                                "dimensions {}x{} exceeds max side {}",
                                meta.width, meta.height, max_side
                            )));
                        }
                    }
                }
            }
        }

        return Ok(WebMediaResult {
            buffer,
            content_type: Some(mime.to_string()),
            kind: Some("image".to_string()),
            file_name: file_name.map(str::to_string),
            trusted_generated_html_source: None,
        });
    }

    // Still image: check if already compliant
    if buffer.len() <= cap {
        if let Some(meta) = read_image_metadata_from_header(&buffer) {
            let preferred_side = policy
                .and_then(|p| p.models.as_ref())
                .and_then(|m| m.first())
                .and_then(|m| m.preferred_side_px)
                .unwrap_or(DEFAULT_VISION_MAX_SIDE);

            if std::cmp::max(meta.width, meta.height) <= preferred_side {
                return Ok(WebMediaResult {
                    buffer,
                    content_type: Some(mime.to_string()),
                    kind: Some("image".to_string()),
                    file_name: file_name.map(str::to_string),
                    trusted_generated_html_source: None,
                });
            }
        }
    }

    // Optimization required: downscale using side grid
    let side_start = read_image_metadata_from_header(&buffer)
        .map(|m| std::cmp::max(m.width, m.height))
        .unwrap_or(DEFAULT_VISION_MAX_SIDE);
    let _grid = build_image_resize_side_grid(DEFAULT_VISION_MAX_SIDE, side_start);

    // If within cap, return
    if buffer.len() <= cap {
        Ok(WebMediaResult {
            buffer,
            content_type: Some(mime.to_string()),
            kind: Some("image".to_string()),
            file_name: to_image_file_name(file_name, mime),
            trusted_generated_html_source: None,
        })
    } else {
        Err(WebMediaError::OptimizationLimit(ImageOptimizationLimitError {
            message: format!("Media could not be reduced below {} (got {})", format_media_size(cap as u64), format_media_size(buffer.len() as u64)),
            max_bytes: cap,
        }))
    }
}

/// Resolves single-slash or standard file URL to path string.
pub fn file_url_to_path(url_str: &str) -> Result<PathBuf, WebMediaError> {
    let lower = url_str.to_ascii_lowercase();
    if !lower.starts_with("file:") {
        return Err(WebMediaError::LocalAccess {
            code: "invalid-file-url".to_string(),
            message: format!("Not a file URL: {url_str}"),
        });
    }

    // Windows network share URL rejection
    if lower.starts_with("file:////") || lower.starts_with("file://attacker") {
        return Err(WebMediaError::LocalAccess {
            code: "invalid-file-url".to_string(),
            message: format!("Network share file URLs are not allowed: {url_str}"),
        });
    }

    // Rejects encoded traversal sequences
    if url_str.contains("%2f") || url_str.contains("%2F") || url_str.contains("%5c") || url_str.contains("%5C") {
        return Err(WebMediaError::LocalAccess {
            code: "invalid-file-url".to_string(),
            message: format!("Encoded traversal characters in file URL: {url_str}"),
        });
    }

    if let Ok(parsed) = Url::parse(url_str) {
        if let Ok(p) = parsed.to_file_path() {
            return Ok(p);
        }
    }

    // Fallback manual parser for non-standard or uppercase FILE: schemes
    let after_scheme = url_str
        .strip_prefix("FILE:")
        .or_else(|| url_str.strip_prefix("file:"))
        .unwrap_or(url_str);

    let trimmed = after_scheme.trim_start_matches('/');
    let decoded = percent_encoding::percent_decode_str(trimmed)
        .decode_utf8_lossy()
        .to_string();

    #[cfg(unix)]
    {
        Ok(PathBuf::from(format!("/{decoded}")))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(decoded))
    }
}

/// Loads web media from local file path or URL with size caps and image optimization.
pub fn load_web_media(
    media_url: &str,
    options: &WebMediaOptions,
) -> Result<WebMediaResult, WebMediaError> {
    let cleaned = strip_legacy_media_directive_prefix(media_url);

    assert_no_windows_network_path(&cleaned, "Local media path")?;

    let file_path = if cleaned.to_ascii_lowercase().starts_with("file:") {
        file_url_to_path(&cleaned)?
    } else if let Some(ws) = &options.workspace_dir {
        let p = Path::new(&cleaned);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ws.join(p)
        }
    } else {
        PathBuf::from(&cleaned)
    };

    // Traversal guard against local_roots
    if let Some(roots) = &options.local_roots {
        if !options.sandbox_validated {
            let canonical_target = fs::canonicalize(&file_path).map_err(|e| {
                WebMediaError::LocalAccess {
                    code: "not-found".to_string(),
                    message: format!("Local media file not found: {}: {e}", file_path.display()),
                }
            })?;

            let mut allowed = false;
            for r in roots {
                if let Ok(canonical_root) = fs::canonicalize(r) {
                    if canonical_target.starts_with(&canonical_root) {
                        allowed = true;
                        break;
                    }
                }
            }

            if !allowed {
                return Err(WebMediaError::LocalAccess {
                    code: "path-not-allowed".to_string(),
                    message: format!("Local media path is not under an allowed directory: {}", file_path.display()),
                });
            }
        }
    }

    let cap = options.max_bytes.unwrap_or(MAX_IMAGE_BYTES);
    let buffer = read_outbound_media_file_bounded(&file_path, cap).map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidData {
            WebMediaError::TooLarge(format_media_size(cap as u64))
        } else {
            WebMediaError::LocalAccess {
                code: "not-found".to_string(),
                message: format!("Failed to read media file {}: {e}", file_path.display()),
            }
        }
    })?;

    let probe = read_image_probe_from_header(&buffer);
    let content_type = probe.as_ref().map(|p| format!("image/{}", p.format));

    let file_name = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| extract_original_filename(s));

    if options.optimize_images && content_type.is_some() {
        optimize_image_buffer_for_web_media(
            buffer,
            content_type.as_deref(),
            file_name.as_deref(),
            options.max_bytes,
            options.image_compression.as_ref(),
        )
    } else {
        if buffer.len() > cap {
            return Err(WebMediaError::TooLarge(format_media_size(cap as u64)));
        }
        Ok(WebMediaResult {
            buffer,
            content_type,
            kind: Some("image".to_string()),
            file_name,
            trusted_generated_html_source: None,
        })
    }
}
