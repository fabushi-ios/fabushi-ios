use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::extensions::attachments::media_understanding_service::{
    MAX_TIMER_TIMEOUT_MS, MIN_MEDIA_TIMEOUT_MS,
};

pub const DEFAULT_MAX_INPUT_IMAGES: usize = 10;
pub const DEFAULT_VIDEO_MAX_BYTES: usize = 100 * 1024 * 1024; // 100MB

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MusicGenerationOutputFormat {
    Mp3,
    Wav,
}

impl MusicGenerationOutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "mp3" => Some(Self::Mp3),
            "wav" => Some(Self::Wav),
            _ => None,
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedMusicAsset {
    pub buffer: Vec<u8>,
    pub mime_type: String,
    pub file_name: Option<String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedVideoAsset {
    pub buffer: Option<Vec<u8>>,
    pub url: Option<String>,
    pub mime_type: String,
    pub file_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MusicGenerationModeCapabilities {
    pub max_tracks: Option<usize>,
    pub max_duration_seconds: Option<u32>,
    pub supports_lyrics: bool,
    pub supports_instrumental: bool,
    pub supports_duration: bool,
    pub supports_format: bool,
    pub supported_formats: Vec<MusicGenerationOutputFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MusicGenerationEditCapabilities {
    pub enabled: bool,
    pub max_input_images: Option<usize>,
    pub mode_capabilities: MusicGenerationModeCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MusicGenerationProviderCapabilities {
    pub base: MusicGenerationModeCapabilities,
    pub edit: Option<MusicGenerationEditCapabilities>,
}

pub fn list_supported_music_generation_modes(
    capabilities: &MusicGenerationProviderCapabilities,
) -> Vec<&'static str> {
    let mut modes = vec!["generate"];
    if let Some(ref edit) = capabilities.edit {
        if edit.enabled {
            modes.push("edit");
        }
    }
    modes
}

pub fn resolve_music_generation_mode_capabilities<'a>(
    capabilities: &'a MusicGenerationProviderCapabilities,
    input_image_count: usize,
) -> Option<&'a MusicGenerationModeCapabilities> {
    if input_image_count > 0 {
        capabilities.edit.as_ref().map(|e| &e.mode_capabilities)
    } else {
        Some(&capabilities.base)
    }
}

pub fn resolve_reference_image_capability_error(
    candidate_ref: &str,
    input_image_count: usize,
    edit_capabilities: Option<&MusicGenerationEditCapabilities>,
) -> Option<String> {
    if input_image_count == 0 {
        return None;
    }
    let edit = match edit_capabilities {
        Some(e) if e.enabled => e,
        _ => {
            return Some(format!(
                "{} does not support reference-image edit inputs",
                candidate_ref
            ));
        }
    };
    let max = edit.max_input_images.unwrap_or(DEFAULT_MAX_INPUT_IMAGES);
    if input_image_count > max {
        let plural = if max == 1 { "" } else { "s" };
        Some(format!(
            "{} supports at most {} reference image{}, {} requested",
            candidate_ref, max, plural, input_image_count
        ))
    } else {
        None
    }
}

pub fn resolve_media_provider_request_timeout_ms(
    timeout_ms: Option<u64>,
    provider_default_timeout_ms: Option<u64>,
) -> Option<u64> {
    let raw = timeout_ms.or(provider_default_timeout_ms)?;
    if raw == 0 {
        None
    } else {
        let clamped = raw.clamp(MIN_MEDIA_TIMEOUT_MS, MAX_TIMER_TIMEOUT_MS);
        Some(clamped)
    }
}

pub fn greatest_common_divisor(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    if a == 0 { 1 } else { a }
}

pub fn derive_aspect_ratio_from_size(size: &str) -> Option<String> {
    let (w, h) = parse_size_dimensions(size)?;
    let gcd = greatest_common_divisor(w as u64, h as u64);
    Some(format!("{}:{}", (w as u64) / gcd, (h as u64) / gcd))
}

pub fn parse_aspect_ratio_value(raw: &str) -> Option<(f64, f64, f64)> {
    let trimmed = raw.trim();
    let parts: Vec<&str> = trimmed.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let w: f64 = parts[0].trim().parse().ok()?;
    let h: f64 = parts[1].trim().parse().ok()?;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some((w, h, w / h))
}

pub fn parse_size_dimensions(raw: &str) -> Option<(u32, u32)> {
    let trimmed = raw.trim();
    let parts: Vec<&str> = trimmed.split(|c| c == 'x' || c == 'X').collect();
    if parts.len() != 2 {
        return None;
    }
    let w: u32 = parts[0].trim().parse().ok()?;
    let h: u32 = parts[1].trim().parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

pub fn resolve_closest_aspect_ratio(
    requested_aspect_ratio: Option<&str>,
    requested_size: Option<&str>,
    supported_aspect_ratios: &[String],
) -> Option<String> {
    if supported_aspect_ratios.is_empty() {
        return requested_aspect_ratio
            .map(str::to_string)
            .or_else(|| requested_size.and_then(derive_aspect_ratio_from_size));
    }
    if let Some(req) = requested_aspect_ratio {
        if supported_aspect_ratios.iter().any(|s| s == req) {
            return Some(req.to_string());
        }
    }
    let req_val = requested_aspect_ratio
        .and_then(parse_aspect_ratio_value)
        .or_else(|| {
            requested_size
                .and_then(derive_aspect_ratio_from_size)
                .as_deref()
                .and_then(parse_aspect_ratio_value)
        })?;

    let mut best: Option<(String, f64, f64)> = None;
    for cand in supported_aspect_ratios {
        if let Some((cw, ch, cval)) = parse_aspect_ratio_value(cand) {
            let primary = (cval / req_val.2).ln().abs();
            let secondary = (cw * req_val.1 - req_val.0 * ch).abs();
            match best {
                None => best = Some((cand.clone(), primary, secondary)),
                Some((ref _bname, bprim, bsec)) => {
                    if primary < bprim || (primary == bprim && secondary < bsec) {
                        best = Some((cand.clone(), primary, secondary));
                    }
                }
            }
        }
    }
    best.map(|(name, _, _)| name)
}

pub fn resolve_closest_size(
    requested_size: Option<&str>,
    requested_aspect_ratio: Option<&str>,
    supported_sizes: &[String],
) -> Option<String> {
    if supported_sizes.is_empty() {
        return requested_size.map(str::to_string);
    }
    if let Some(req) = requested_size {
        if supported_sizes.iter().any(|s| s == req) {
            return Some(req.to_string());
        }
    }
    let req_size = requested_size.and_then(parse_size_dimensions);
    let req_ar = requested_aspect_ratio.and_then(parse_aspect_ratio_value);
    if req_size.is_none() && req_ar.is_none() {
        return None;
    }
    let target_ar = if let Some((w, h)) = req_size {
        (w as f64) / (h as f64)
    } else {
        req_ar.unwrap().2
    };
    let target_area = req_size.map(|(w, h)| (w as f64) * (h as f64));

    let mut best: Option<(String, f64, f64)> = None;
    for cand in supported_sizes {
        if let Some((cw, ch)) = parse_size_dimensions(cand) {
            let cand_ar = (cw as f64) / (ch as f64);
            let cand_area = (cw as f64) * (ch as f64);
            let primary = (cand_ar / target_ar).ln().abs();
            let secondary = match target_area {
                Some(ta) => (cand_area / ta).ln().abs(),
                None => cand_area,
            };
            match best {
                None => best = Some((cand.clone(), primary, secondary)),
                Some((ref _bname, bprim, bsec)) => {
                    if primary < bprim || (primary == bprim && secondary < bsec) {
                        best = Some((cand.clone(), primary, secondary));
                    }
                }
            }
        }
    }
    best.map(|(name, _, _)| name)
}

pub fn parse_resolution_rank(resolution: &str) -> Option<(f64, &'static str)> {
    let trimmed = resolution.trim();
    if trimmed.is_empty() {
        return None;
    }
    let last = trimmed.chars().last()?.to_ascii_uppercase();
    let num_part = &trimmed[..trimmed.len() - 1];
    let val: f64 = num_part.trim().parse().ok()?;
    if !val.is_finite() || val <= 0.0 {
        return None;
    }
    if last == 'K' {
        Some((val * 1000.0, "K"))
    } else if last == 'P' {
        Some((val, "P"))
    } else {
        None
    }
}

pub fn resolve_closest_resolution(
    requested_resolution: Option<&str>,
    supported_resolutions: &[String],
) -> Option<String> {
    if supported_resolutions.is_empty() {
        return requested_resolution.map(str::to_string);
    }
    if let Some(req) = requested_resolution {
        if supported_resolutions.iter().any(|s| s == req) {
            return Some(req.to_string());
        }
    }
    let req_rank = requested_resolution.and_then(parse_resolution_rank)?;

    let mut best: Option<(String, f64, u8)> = None;
    for cand in supported_resolutions {
        if let Some((cand_val, cand_unit)) = parse_resolution_rank(cand) {
            if cand_unit != req_rank.1 {
                continue;
            }
            let primary = (cand_val - req_rank.0).abs();
            let secondary = if cand_val < req_rank.0 { 1 } else { 0 };
            match best {
                None => best = Some((cand.clone(), primary, secondary)),
                Some((ref _bname, bprim, bsec)) => {
                    if primary < bprim || (primary == bprim && secondary < bsec) {
                        best = Some((cand.clone(), primary, secondary));
                    }
                }
            }
        }
    }
    best.map(|(name, _, _)| name)
}

pub fn normalize_duration_to_closest_max(
    duration_seconds: Option<f64>,
    max_duration_seconds: Option<f64>,
) -> Option<u32> {
    let dur = duration_seconds?;
    if !dur.is_finite() || dur <= 0.0 {
        return None;
    }
    let rounded = (dur.round() as i64).max(1) as u32;
    match max_duration_seconds {
        Some(max) if max.is_finite() && max > 0.0 => {
            let max_rounded = (max.round() as i64).max(1) as u32;
            Some(rounded.min(max_rounded))
        }
        _ => Some(rounded),
    }
}

pub fn read_generated_video_asset(
    buffer: Vec<u8>,
    content_type: Option<&str>,
    index: Option<usize>,
    max_bytes: Option<usize>,
    overflow_url: Option<&str>,
) -> Result<GeneratedVideoAsset, String> {
    let mime = content_type.unwrap_or("video/mp4").to_string();
    let ext = match mime.as_str() {
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "mp4",
    };
    let file_name = format!("video-{}.{}", index.unwrap_or(0) + 1, ext);
    let limit = max_bytes.unwrap_or(DEFAULT_VIDEO_MAX_BYTES);

    if buffer.len() > limit {
        if let Some(url) = overflow_url {
            return Ok(GeneratedVideoAsset {
                buffer: None,
                url: Some(url.to_string()),
                mime_type: mime,
                file_name,
            });
        }
        return Err(format!(
            "Generated video exceeds byte limit ({} > {})",
            buffer.len(),
            limit
        ));
    }

    Ok(GeneratedVideoAsset {
        buffer: Some(buffer),
        url: None,
        mime_type: mime,
        file_name,
    })
}
