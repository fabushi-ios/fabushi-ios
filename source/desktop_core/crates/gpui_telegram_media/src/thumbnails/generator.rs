use super::geometry::{fit_dimensions_downscale_only, shrink_to_keep_aspect};
use super::stripped::{envelope_stripped_photo, parse_stripped_dimensions};
use gpui_telegram_core::error::{Result, TelegramError};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub has_alpha: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImageRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub format: String,
}

fn format_label(fmt: image::ImageFormat) -> &'static str {
    match fmt {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpeg",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Gif => "gif",
        _ => "image",
    }
}

/// Telegram standard thumbnail size presets.
/// Upstream reference: `Telegram/SourceFiles/storage/localimageloader.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThumbnailPreset {
    /// Micro inline preview (32x32, photoStrippedSize 'i')
    Micro,
    /// Small thumbnail for avatar/grid (100x100, photoSize 's')
    Small,
    /// Medium chat bubble preview (320x320, photoSize 'm')
    Medium,
    /// Large lightbox preview (800x800, photoSize 'x')
    Large,
    /// Extra Large full viewport preview (1280x1280, photoSize 'y')
    ExtraLarge,
}

impl ThumbnailPreset {
    /// Returns the maximum bounding box (width, height) in pixels.
    pub fn max_dimensions(&self) -> (u32, u32) {
        match self {
            ThumbnailPreset::Micro => (32, 32),
            ThumbnailPreset::Small => (100, 100),
            ThumbnailPreset::Medium => (320, 320),
            ThumbnailPreset::Large => (800, 800),
            ThumbnailPreset::ExtraLarge => (1280, 1280),
        }
    }
}

pub struct ThumbnailGenerator;

impl ThumbnailGenerator {
    /// Computes target fitted dimensions using upstream Telegram `shrinkToKeepAspect`.
    pub fn shrink_to_keep_aspect(
        width: u32,
        height: u32,
        to_width: u32,
        to_height: u32,
    ) -> (u32, u32) {
        shrink_to_keep_aspect(width, height, to_width, to_height)
    }

    /// Computes target dimensions within a bounding box, preserving aspect ratio.
    pub fn compute_target_dimensions(
        orig_w: u32,
        orig_h: u32,
        max_w: u32,
        max_h: u32,
    ) -> (u32, u32) {
        if orig_w == 0 || orig_h == 0 {
            return (max_w.max(1), max_h.max(1));
        }
        shrink_to_keep_aspect(orig_w, orig_h, max_w, max_h)
    }

    /// Fits original dimensions into a standard Telegram preset without exceeding bounding box.
    pub fn fit_preset(orig_w: u32, orig_h: u32, preset: ThumbnailPreset) -> (u32, u32) {
        let (max_w, max_h) = preset.max_dimensions();
        shrink_to_keep_aspect(orig_w, orig_h, max_w, max_h)
    }

    /// Fits original dimensions into a standard Telegram preset, downscaling only if necessary.
    pub fn fit_preset_downscale_only(
        orig_w: u32,
        orig_h: u32,
        preset: ThumbnailPreset,
    ) -> (u32, u32) {
        let (max_w, max_h) = preset.max_dimensions();
        fit_dimensions_downscale_only(orig_w, orig_h, max_w, max_h)
    }

    /// Resizes an input image (JPEG, PNG, WebP) to the target preset dimensions using Lanczos3 filter.
    pub fn generate_thumbnail(image_bytes: &[u8], preset: ThumbnailPreset) -> Result<Vec<u8>> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| TelegramError::Validation(format!("Failed to decode image: {e}")))?;

        let (max_w, max_h) = preset.max_dimensions();
        let (target_w, target_h) = shrink_to_keep_aspect(img.width(), img.height(), max_w, max_h);

        let resized = img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);

        let mut output = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut output);
        resized
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| TelegramError::Internal(format!("Failed to encode thumbnail: {e}")))?;

        Ok(output)
    }

    /// Generates a real decodable micro JPEG preview from valid PNG/JPEG/WebP image bytes.
    pub fn extract_thumbnail_placeholder(data: &[u8]) -> Result<Vec<u8>> {
        Self::generate_thumbnail(data, ThumbnailPreset::Micro)
    }

    /// Inspects image dimensions and format across PNG, JPEG, and WebP payloads.
    pub fn probe_metadata(image_bytes: &[u8]) -> Result<ImageMetadata> {
        let fmt = image::guess_format(image_bytes)
            .map_err(|e| TelegramError::Validation(format!("Unrecognized image format: {e}")))?;
        let img = image::load_from_memory_with_format(image_bytes, fmt)
            .map_err(|e| TelegramError::Validation(format!("Failed to decode image: {e}")))?;
        Ok(ImageMetadata {
            width: img.width(),
            height: img.height(),
            format: format_label(fmt).to_string(),
            has_alpha: img.color().has_alpha(),
        })
    }

    /// Decodes PNG, JPEG, or WebP bytes into packed RGBA8 pixel data for GPUI rendering.
    pub fn decode_to_rgba8(image_bytes: &[u8]) -> Result<DecodedImageRgba> {
        let fmt = image::guess_format(image_bytes)
            .map_err(|e| TelegramError::Validation(format!("Unrecognized image format: {e}")))?;
        let img = image::load_from_memory_with_format(image_bytes, fmt)
            .map_err(|e| TelegramError::Validation(format!("Failed to decode image: {e}")))?;
        let width = img.width();
        let height = img.height();
        let rgba = img.to_rgba8().into_raw();
        Ok(DecodedImageRgba {
            width,
            height,
            rgba,
            format: format_label(fmt).to_string(),
        })
    }

    /// Resizes an input image (PNG, JPEG, WebP) and encodes the thumbnail as PNG (preserving alpha).
    pub fn generate_thumbnail_png(image_bytes: &[u8], preset: ThumbnailPreset) -> Result<Vec<u8>> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| TelegramError::Validation(format!("Failed to decode image: {e}")))?;
        let (max_w, max_h) = preset.max_dimensions();
        let (target_w, target_h) = shrink_to_keep_aspect(img.width(), img.height(), max_w, max_h);
        let resized = img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);
        let mut output = Vec::new();
        let mut cursor = Cursor::new(&mut output);
        resized
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| TelegramError::Internal(format!("Failed to encode PNG thumbnail: {e}")))?;
        Ok(output)
    }

    /// Encodes a Telegram stripped inline preview (`0x01`, width, height, JPEG scan bytes).
    pub fn generate_stripped_preview(image_bytes: &[u8]) -> Result<Vec<u8>> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| TelegramError::Validation(format!("Failed to decode image: {e}")))?;
        let (max_w, max_h) = ThumbnailPreset::Micro.max_dimensions();
        let (target_w, target_h) = shrink_to_keep_aspect(img.width(), img.height(), max_w, max_h);
        let resized = img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);
        let mut jpeg_buf = Vec::new();
        let mut cursor = Cursor::new(&mut jpeg_buf);
        resized
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .map_err(|e| TelegramError::Internal(format!("Failed to encode stripped preview: {e}")))?;
        Ok(envelope_stripped_photo(
            target_w.clamp(1, 255) as u8,
            target_h.clamp(1, 255) as u8,
            &jpeg_buf,
        ))
    }

    /// Decodes a Telegram stripped inline preview (`0x01 || w || h || jpeg`) or micro JPEG into RGBA8.
    pub fn decode_stripped_preview(stripped_bytes: &[u8]) -> Result<DecodedImageRgba> {
        if stripped_bytes.first().copied() == Some(super::stripped::TELEGRAM_STRIPPED_PREFIX) {
            let (_w, _h) = parse_stripped_dimensions(stripped_bytes)?;
            return Self::decode_to_rgba8(&stripped_bytes[3..]);
        }
        Self::decode_to_rgba8(stripped_bytes)
    }
}
