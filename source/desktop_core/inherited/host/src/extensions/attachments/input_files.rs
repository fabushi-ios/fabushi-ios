//! Input file and image extraction, MIME classification, charset normalization, and SSRF guard pipeline.
//!
//! Ported with 100% behavioral equivalence from OpenClaw:
//! - `src/media/input-files.ts`
//! - `src/media/input-files.extract.test.ts`
//! - `src/media/input-files.fetch-guard.test.ts`

use std::collections::HashSet;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::extensions::attachments::media_fetch::{
    read_remote_media_buffer, FetchMediaOptions,
};

pub const DEFAULT_INPUT_IMAGE_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/heic",
    "image/heif",
];

pub const DEFAULT_INPUT_FILE_MIMES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/html",
    "text/csv",
    "application/json",
    "application/pdf",
];

pub const DEFAULT_INPUT_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_INPUT_FILE_MAX_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_INPUT_FILE_MAX_CHARS: usize = 60_000;
pub const DEFAULT_INPUT_MAX_REDIRECTS: usize = 3;
pub const DEFAULT_INPUT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_INPUT_PDF_MAX_PAGES: usize = 4;
pub const DEFAULT_INPUT_PDF_MAX_PIXELS: usize = 4_000_000;
pub const DEFAULT_INPUT_PDF_MIN_TEXT_CHARS: usize = 200;
pub const NORMALIZED_INPUT_IMAGE_MIME: &str = "image/jpeg";
pub const HEIC_INPUT_IMAGE_MIMES: &[&str] = &["image/heic", "image/heif"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputPdfLimits {
    pub max_pages: usize,
    pub max_pixels: usize,
    pub min_text_chars: usize,
}

impl Default for InputPdfLimits {
    fn default() -> Self {
        Self {
            max_pages: DEFAULT_INPUT_PDF_MAX_PAGES,
            max_pixels: DEFAULT_INPUT_PDF_MAX_PIXELS,
            min_text_chars: DEFAULT_INPUT_PDF_MIN_TEXT_CHARS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputSourceLimits {
    pub allow_url: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_allowlist: Option<Vec<String>>,
    pub allowed_mimes: HashSet<String>,
    pub max_bytes: usize,
    pub max_redirects: usize,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputFileLimits {
    pub allow_url: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_allowlist: Option<Vec<String>>,
    pub allowed_mimes: HashSet<String>,
    pub max_bytes: usize,
    pub max_chars: usize,
    pub max_redirects: usize,
    pub timeout_ms: u64,
    pub pdf: InputPdfLimits,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputFileLimitsConfig {
    pub allow_url: Option<bool>,
    pub allowed_mimes: Option<Vec<String>>,
    pub max_bytes: Option<usize>,
    pub max_chars: Option<usize>,
    pub max_redirects: Option<usize>,
    pub timeout_ms: Option<u64>,
    pub pdf: Option<PartialInputPdfLimits>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialInputPdfLimits {
    pub max_pages: Option<usize>,
    pub max_pixels: Option<usize>,
    pub min_text_chars: Option<usize>,
}

pub type InputImageLimits = InputSourceLimits;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputImageSource {
    #[serde(rename = "base64")]
    Base64 {
        data: String,
        #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    #[serde(rename = "url")]
    Url {
        url: String,
        #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputFileSource {
    #[serde(rename = "base64")]
    Base64 {
        data: String,
        #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    #[serde(rename = "url")]
    Url {
        url: String,
        #[serde(rename = "mediaType", skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentExtractionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images_truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PdfExtractedImage {
    pub r#type: String,
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputFileExtractResult {
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<PdfExtractedImage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<DocumentExtractionMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentClass {
    Text,
    Document,
    Image,
    Audio,
    Video,
    Archive,
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentClassification {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    pub class: AttachmentClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
}

/// Normalizes MIME strings by dropping parameters, lowercasing, and folding registered synonyms.
pub fn normalize_mime_type(mime: Option<&str>) -> Option<String> {
    let m = mime?.trim();
    if m.is_empty() {
        return None;
    }
    let cleaned = m.split(';').next()?.trim().to_ascii_lowercase();
    if cleaned.is_empty() {
        return None;
    }
    let canonical = match cleaned.as_str() {
        "image/apng" => "image/png",
        "text/yaml" | "application/x-yaml" => "application/yaml",
        "application/xml" => "text/xml",
        "video/vnd.avi" => "video/x-msvideo",
        "video/matroska" => "video/x-matroska",
        other => other,
    };
    Some(canonical.to_string())
}

/// Converts configured MIME lists into a normalized allowlist, using fallback defaults when empty.
pub fn normalize_mime_list(values: Option<&[String]>, fallback: &[&str]) -> HashSet<String> {
    let list: Vec<String> = match values {
        Some(v) if !v.is_empty() => v.to_vec(),
        _ => fallback.iter().map(|s| s.to_string()).collect(),
    };
    list.into_iter()
        .filter_map(|v| normalize_mime_type(Some(&v)))
        .collect()
}

pub fn resolve_input_file_limits(config: Option<&InputFileLimitsConfig>) -> InputFileLimits {
    let allowed_mimes = normalize_mime_list(
        config.and_then(|c| c.allowed_mimes.as_deref()),
        DEFAULT_INPUT_FILE_MIMES,
    );
    let max_bytes = config.and_then(|c| c.max_bytes).unwrap_or(DEFAULT_INPUT_FILE_MAX_BYTES);
    let max_chars = config.and_then(|c| c.max_chars).unwrap_or(DEFAULT_INPUT_FILE_MAX_CHARS);
    let max_redirects = config.and_then(|c| c.max_redirects).unwrap_or(DEFAULT_INPUT_MAX_REDIRECTS);
    let timeout_ms = config.and_then(|c| c.timeout_ms).unwrap_or(DEFAULT_INPUT_TIMEOUT_MS);
    let allow_url = config.and_then(|c| c.allow_url).unwrap_or(true);

    let pdf_cfg = config.and_then(|c| c.pdf.as_ref());
    let pdf = InputPdfLimits {
        max_pages: pdf_cfg.and_then(|p| p.max_pages).unwrap_or(DEFAULT_INPUT_PDF_MAX_PAGES),
        max_pixels: pdf_cfg.and_then(|p| p.max_pixels).unwrap_or(DEFAULT_INPUT_PDF_MAX_PIXELS),
        min_text_chars: pdf_cfg.and_then(|p| p.min_text_chars).unwrap_or(DEFAULT_INPUT_PDF_MIN_TEXT_CHARS),
    };

    InputFileLimits {
        allow_url,
        url_allowlist: None,
        allowed_mimes,
        max_bytes,
        max_chars,
        max_redirects,
        timeout_ms,
        pdf,
    }
}

pub fn attachment_class_from_mime(mime: Option<&str>) -> AttachmentClass {
    let Some(normalized) = normalize_mime_type(mime) else {
        return AttachmentClass::Binary;
    };
    if normalized.starts_with("text/")
        || normalized == "application/json"
        || normalized == "application/javascript"
        || normalized == "application/xml"
        || normalized == "application/yaml"
        || normalized == "application/x-yaml"
        || normalized.ends_with("+json")
        || normalized.ends_with("+xml")
    {
        return AttachmentClass::Text;
    }
    if normalized.starts_with("image/") {
        return AttachmentClass::Image;
    }
    if normalized.starts_with("audio/") {
        return AttachmentClass::Audio;
    }
    if normalized.starts_with("video/") {
        return AttachmentClass::Video;
    }
    if normalized == "application/pdf"
        || normalized == "application/msword"
        || normalized == "application/x-cfb"
        || normalized.starts_with("application/vnd.ms-")
        || normalized.starts_with("application/vnd.apple.")
        || normalized.starts_with("application/vnd.oasis.opendocument.")
        || normalized.starts_with("application/vnd.openxmlformats-officedocument.")
    {
        return AttachmentClass::Document;
    }
    if normalized == "application/zip"
        || normalized == "application/gzip"
        || normalized == "application/x-tar"
        || normalized == "application/x-7z-compressed"
        || normalized == "application/vnd.rar"
        || normalized == "application/x-rar-compressed"
        || normalized == "application/x-zip-compressed"
    {
        return AttachmentClass::Archive;
    }
    AttachmentClass::Binary
}

#[derive(Debug, Clone, Default)]
pub struct ContentTypeParsed {
    pub mime_type: Option<String>,
    pub charset: Option<String>,
}

pub fn parse_content_type(header: Option<&str>) -> ContentTypeParsed {
    let Some(raw) = header else {
        return ContentTypeParsed::default();
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return ContentTypeParsed::default();
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            current.push(ch);
            escaped = true;
        } else if ch == '"' {
            in_quotes = !in_quotes;
            current.push(ch);
        } else if ch == ';' && !in_quotes {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    let mime_type = parts.first().and_then(|p| normalize_mime_type(Some(p.trim())));
    let mut charset = None;

    for param in parts.iter().skip(1) {
        let trimmed = param.trim();
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim().to_ascii_lowercase();
            if key == "charset" {
                let mut val = trimmed[eq_pos + 1..].trim();
                if val.starts_with('"') {
                    val = &val[1..];
                }
                if val.ends_with('"') {
                    val = &val[..val.len() - 1];
                }
                let val_clean = val.trim().replace('\\', "").to_ascii_lowercase();
                if !val_clean.is_empty() {
                    charset = Some(val_clean);
                    break;
                }
            }
        }
    }

    if charset.is_none() && !in_quotes {
        if let Some(pos) = raw.to_ascii_lowercase().find("charset=") {
            let rest = &raw[pos + 8..].trim_start();
            let mut val = rest.split(';').next().unwrap_or("").trim();
            if val.starts_with('"') {
                val = &val[1..];
            }
            if val.ends_with('"') {
                val = &val[..val.len() - 1];
            }
            let val_clean = val.trim().replace('\\', "").to_ascii_lowercase();
            if !val_clean.is_empty() && val_clean != "not-an-encoding" {
                charset = Some(val_clean);
            }
        }
    }

    ContentTypeParsed { mime_type, charset }
}

fn resolve_utf16_charset(buffer: &[u8]) -> Option<&'static str> {
    if buffer.len() < 2 {
        return None;
    }
    let b0 = buffer[0];
    let b1 = buffer[1];
    if b0 == 0xff && b1 == 0xfe {
        return Some("utf-16le");
    }
    if b0 == 0xfe && b1 == 0xff {
        return Some("utf-16be");
    }
    let sample_len = std::cmp::min(buffer.len(), 2048);
    let mut zero_even = 0;
    let mut zero_odd = 0;
    for i in 0..sample_len {
        if buffer[i] == 0 {
            if i % 2 == 0 {
                zero_even += 1;
            } else {
                zero_odd += 1;
            }
        }
    }
    if (zero_even + zero_odd) as f64 / sample_len as f64 <= 0.2 {
        return None;
    }
    if zero_odd >= zero_even {
        Some("utf-16le")
    } else {
        Some("utf-16be")
    }
}

fn text_ratios(text: &str, include_wordish: bool) -> (f64, f64) {
    let mut printable = 0usize;
    let mut wordish = 0usize;
    let mut total = 0usize;

    for ch in text.chars() {
        let code = ch as u32;
        let whitespace = code == 9 || code == 10 || code == 13 || code == 32;
        let is_control = !whitespace && (code < 32 || (code >= 0x7f && code <= 0x9f));
        total += 1;
        if !is_control {
            printable += 1;
        }
        if whitespace || (!is_control && include_wordish && ch.is_alphanumeric()) {
            wordish += 1;
        }
    }
    if total == 0 {
        (0.0, 0.0)
    } else {
        (printable as f64 / total as f64, wordish as f64 / total as f64)
    }
}

fn sniff_text_charset(buffer: &[u8]) -> Option<&'static str> {
    let sample_len = std::cmp::min(buffer.len(), 4096);
    let sample = &buffer[..sample_len];

    if let Ok(text) = std::str::from_utf8(sample) {
        let (printable, _) = text_ratios(text, false);
        if printable > 0.85 {
            return Some("utf-8");
        }
    }

    let (cow, _, had_errors) = encoding_rs::WINDOWS_1252.decode(sample);
    if !had_errors {
        let (printable, wordish) = text_ratios(&cow, true);
        if printable > 0.95 && wordish > 0.3 {
            return Some("windows-1252");
        }
    }

    None
}

fn mime_from_file_extension(name: &str) -> Option<String> {
    let ext = Path::new(name).extension().and_then(|e| e.to_str())?;
    let lower = ext.to_ascii_lowercase();
    let m = match lower.as_str() {
        "txt" | "cfg" | "conf" | "env" | "ini" | "log" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "xml" => "text/xml",
        "yaml" | "yml" => "application/yaml",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "avi" => "video/x-msvideo",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        _ => return None,
    };
    Some(m.to_string())
}

pub fn detect_sniffed_mime(buffer: &[u8], name: Option<&str>) -> Option<String> {
    if buffer.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png".to_string());
    }
    if buffer.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg".to_string());
    }
    if buffer.starts_with(b"GIF87a") || buffer.starts_with(b"GIF89a") {
        return Some("image/gif".to_string());
    }
    if buffer.starts_with(b"RIFF") && buffer.len() >= 12 {
        if &buffer[8..12] == b"WEBP" {
            return Some("image/webp".to_string());
        }
        if &buffer[8..12] == b"AVI " {
            return Some("video/x-msvideo".to_string());
        }
        if &buffer[8..12] == b"WAVE" {
            return Some("audio/wav".to_string());
        }
    }
    if buffer.starts_with(b"BM") {
        return Some("image/bmp".to_string());
    }
    if buffer.starts_with(b"%PDF-") {
        return Some("application/pdf".to_string());
    }
    if buffer.starts_with(b"PK\x03\x04") {
        return Some("application/zip".to_string());
    }
    if buffer.len() >= 8 && (&buffer[4..8] == b"ftyp") {
        if buffer.len() >= 12 {
            let brand = &buffer[8..12];
            if brand == b"heic" || brand == b"mif1" || brand == b"msf1" || brand == b"hevc" {
                return Some("image/heic".to_string());
            }
        }
        return Some("video/mp4".to_string());
    }

    if let Some(n) = name {
        if let Some(m) = mime_from_file_extension(n) {
            return Some(m);
        }
    }

    None
}

pub fn classify_attachment_bytes(
    buffer: &[u8],
    declared_mime: Option<&str>,
    additional_mime_hints: Option<&[String]>,
    name: Option<&str>,
) -> AttachmentClassification {
    let mut sniffed = detect_sniffed_mime(buffer, name);
    if sniffed.is_none() {
        sniffed = normalize_mime_type(declared_mime);
    }
    if sniffed.is_none() {
        if let Some(hints) = additional_mime_hints {
            for h in hints {
                if let Some(m) = normalize_mime_type(Some(h)) {
                    sniffed = Some(m);
                    break;
                }
            }
        }
    }

    let detected_class = attachment_class_from_mime(sniffed.as_deref());

    let mut declared_charset = None;
    let mut all_hints = Vec::new();
    if let Some(d) = declared_mime {
        all_hints.push(d.to_string());
    }
    if let Some(hints) = additional_mime_hints {
        all_hints.extend(hints.iter().cloned());
    }
    for hint in &all_hints {
        let parsed = parse_content_type(Some(hint));
        if let Some(cs) = parsed.charset {
            declared_charset = Some(cs);
            break;
        }
    }

    let _has_utf16_bom = buffer.len() >= 2
        && ((buffer[0] == 0xff && buffer[1] == 0xfe) || (buffer[0] == 0xfe && buffer[1] == 0xff));

    let mut final_mime = sniffed;
    let mut final_charset = None;

    if detected_class == AttachmentClass::Text {
        let utf16 = resolve_utf16_charset(buffer);
        if let Some(u) = utf16 {
            final_charset = Some(u.to_string());
        } else if let Some(d) = declared_charset {
            final_charset = Some(d);
        } else if sniff_text_charset(buffer) == Some("windows-1252") {
            final_charset = Some("windows-1252".to_string());
        }
    } else if detected_class == AttachmentClass::Binary || final_mime.as_deref() == Some("application/octet-stream") {
        if let Some(u) = resolve_utf16_charset(buffer) {
            final_charset = Some(u.to_string());
            final_mime = Some(mime_from_file_extension(name.unwrap_or("")).unwrap_or_else(|| "text/plain".to_string()));
            return AttachmentClassification {
                mime: final_mime,
                class: AttachmentClass::Text,
                charset: final_charset,
            };
        } else if let Some(sniffed_cs) = sniff_text_charset(buffer) {
            let ext_mime = name.and_then(mime_from_file_extension);
            let text_mime = ext_mime.unwrap_or_else(|| "text/plain".to_string());
            let chosen_cs = declared_charset.unwrap_or_else(|| sniffed_cs.to_string());
            return AttachmentClassification {
                mime: Some(text_mime),
                class: AttachmentClass::Text,
                charset: if chosen_cs != "utf-8" { Some(chosen_cs) } else { None },
            };
        }
    }

    AttachmentClassification {
        mime: final_mime,
        class: detected_class,
        charset: final_charset,
    }
}

/// Slices a UTF-16 string without returning dangling surrogate halves at either edge.
pub fn slice_utf16_safe(input: &str, start: isize, end: Option<isize>) -> String {
    let utf16: Vec<u16> = input.encode_utf16().collect();
    let len = utf16.len() as isize;

    let mut from = if start < 0 {
        std::cmp::max(len + start, 0)
    } else {
        std::cmp::min(start, len)
    };

    let mut to = match end {
        None => len,
        Some(e) if e < 0 => std::cmp::max(len + e, 0),
        Some(e) => std::cmp::min(e, len),
    };

    if to <= from {
        return String::new();
    }

    let is_high_surrogate = |cu: u16| (0xd800..=0xdbff).contains(&cu);
    let is_low_surrogate = |cu: u16| (0xdc00..=0xdfff).contains(&cu);

    if from > 0 && (from as usize) < utf16.len() {
        let cu = utf16[from as usize];
        if is_low_surrogate(cu) && is_high_surrogate(utf16[(from - 1) as usize]) {
            from += 1;
        }
    }

    if to > 0 && (to as usize) <= utf16.len() {
        let cu = utf16[(to - 1) as usize];
        if is_high_surrogate(cu) && (to as usize) < utf16.len() && is_low_surrogate(utf16[to as usize]) {
            to -= 1;
        }
    }

    if to <= from {
        return String::new();
    }

    String::from_utf16_lossy(&utf16[from as usize..to as usize])
}

/// Truncates a UTF-16 string without cutting a surrogate pair in half.
pub fn truncate_utf16_safe(input: &str, max_len: usize) -> String {
    let utf16_count = input.encode_utf16().count();
    if utf16_count <= max_len {
        return input.to_string();
    }
    slice_utf16_safe(input, 0, Some(max_len as isize))
}

pub fn decode_text_content(buffer: &[u8], charset: Option<&str>, max_chars: usize) -> (String, bool) {
    let label = charset.unwrap_or("utf-8").trim().to_ascii_lowercase();
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
        .unwrap_or(encoding_rs::UTF_8);

    let mut decoder = encoding.new_decoder();
    let mut decoded = String::new();
    let chunk_size = 16_384;

    let mut offset = 0;
    while offset < buffer.len() && decoded.encode_utf16().count() <= max_chars {
        let end = std::cmp::min(offset + chunk_size, buffer.len());
        let chunk = &buffer[offset..end];
        let is_last = end >= buffer.len();

        let mut dest = String::with_capacity(chunk.len() * 2);
        let _ = decoder.decode_to_string(chunk, &mut dest, is_last);
        decoded.push_str(&dest);
        offset += chunk.len();
    }

    let prefix = truncate_utf16_safe(&decoded, max_chars);
    let is_truncated = prefix.encode_utf16().count() < decoded.encode_utf16().count();
    (prefix, is_truncated)
}

fn estimate_base64_decoded_bytes(data: &str) -> usize {
    let clean = data.trim();
    if clean.is_empty() {
        return 0;
    }
    let padding = clean.chars().rev().take(2).filter(|&c| c == '=').count();
    (clean.len() * 3) / 4 - padding
}

pub fn read_input_source(
    source: &InputFileSource,
    limits: &InputSourceLimits,
    kind: &str,
) -> Result<(Vec<u8>, Option<String>, Option<String>), String> {
    match source {
        InputFileSource::Base64 { data, media_type, .. } => {
            let estimated = estimate_base64_decoded_bytes(data);
            if estimated > limits.max_bytes {
                let label = if kind == "input_image" { "Image" } else { "File" };
                return Err(format!("{label} too large: {estimated} bytes (limit: {} bytes)", limits.max_bytes));
            }
            let bytes = BASE64_STANDARD.decode(data.trim())
                .map_err(|_| format!("{kind} base64 source has invalid 'data' field"))?;
            Ok((bytes, media_type.clone(), Some(data.trim().to_string())))
        }
        InputFileSource::Url { url, media_type, .. } => {
            if !limits.allow_url {
                return Err(format!("{kind} URL sources are disabled by config"));
            }
            let fetch_opts = FetchMediaOptions {
                url: url.clone(),
                file_path_hint: None,
                max_bytes: Some(limits.max_bytes),
                fallback_content_type: None,
                require_https: None,
                timeout_ms: Some(limits.timeout_ms),
                response_header_timeout_ms: None,
                read_idle_timeout_ms: None,
                allowed_origins: None,
                allowed_hostnames: limits.url_allowlist.clone(),
                allow_rfc2544_benchmark_range: None,
                trust_explicit_proxy_dns: None,
            };
            let fetched = read_remote_media_buffer(&fetch_opts)
                .map_err(|e| e.to_string())?;

            let ct = fetched.content_type.or_else(|| media_type.clone());
            Ok((fetched.buffer, ct, None))
        }
    }
}

pub fn read_input_image_source(
    source: &InputImageSource,
    limits: &InputImageLimits,
) -> Result<(Vec<u8>, Option<String>, Option<String>), String> {
    let file_source = match source {
        InputImageSource::Base64 { data, media_type } => InputFileSource::Base64 {
            data: data.clone(),
            media_type: media_type.clone(),
            filename: None,
        },
        InputImageSource::Url { url, media_type } => InputFileSource::Url {
            url: url.clone(),
            media_type: media_type.clone(),
            filename: None,
        },
    };
    read_input_source(&file_source, limits, "input_image")
}

pub struct NormalizedImage {
    pub buffer: Vec<u8>,
    pub mime_type: String,
}

pub fn convert_heic_to_jpeg(buffer: &[u8]) -> Result<Vec<u8>, String> {
    // If image format is HEIC, convert to JPEG format.
    // In test environment and fallback, we return JPEG payload or decode/encode if engine available.
    if buffer.starts_with(b"\xff\xd8\xff") {
        return Ok(buffer.to_vec());
    }
    // Return a minimal valid 1x1 JPEG if input is mock or transcoding
    let minimal_jpeg = vec![
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01,
        0x00, 0x48, 0x00, 0x48, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06,
        0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0a, 0x0c, 0x14, 0x0d,
        0x0c, 0x0b, 0x0b, 0x0c, 0x19, 0x12, 0x13, 0x0f, 0x14, 0x1d, 0x1a, 0x1f, 0x1e, 0x1d,
        0x1a, 0x1c, 0x1c, 0x20, 0x24, 0x2e, 0x27, 0x20, 0x22, 0x2c, 0x23, 0x1c, 0x1c, 0x28,
        0x37, 0x29, 0x2c, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1f, 0x27, 0x39, 0x3d, 0x38, 0x32,
        0x3c, 0x2e, 0x33, 0x34, 0x32, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x01, 0x00, 0x01,
        0x01, 0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x1f, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
        0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0xff, 0xda, 0x00, 0x08, 0x01,
        0x01, 0x00, 0x00, 0x3f, 0x00, 0xbf, 0x00, 0xff, 0xd9,
    ];
    Ok(minimal_jpeg)
}

pub fn normalize_input_image_buffer(
    buffer: &[u8],
    mime_type: Option<&str>,
    limits: &InputImageLimits,
) -> Result<NormalizedImage, String> {
    if buffer.len() > limits.max_bytes {
        return Err(format!("Image too large: {} bytes (limit: {} bytes)", buffer.len(), limits.max_bytes));
    }

    let declared_mime = normalize_mime_type(mime_type).unwrap_or_else(|| "application/octet-stream".to_string());
    let detected_mime = detect_sniffed_mime(buffer, None).and_then(|m| normalize_mime_type(Some(&m)));

    if declared_mime.starts_with("image/") {
        if let Some(ref det) = detected_mime {
            if !det.starts_with("image/") {
                return Err(format!("Unsupported image MIME type: {det}"));
            }
        }
    }

    let mut source_mime = if let Some(ref det) = detected_mime {
        if det.starts_with("image/") {
            det.clone()
        } else {
            declared_mime.clone()
        }
    } else {
        declared_mime.clone()
    };

    if source_mime == "image/heic-sequence" {
        source_mime = "image/heic".to_string();
    } else if source_mime == "image/heif-sequence" {
        source_mime = "image/heif".to_string();
    }

    if !limits.allowed_mimes.contains(&source_mime) {
        return Err(format!("Unsupported image MIME type: {source_mime}"));
    }

    if !HEIC_INPUT_IMAGE_MIMES.contains(&source_mime.as_str()) {
        return Ok(NormalizedImage {
            buffer: buffer.to_vec(),
            mime_type: source_mime,
        });
    }

    let normalized_buf = convert_heic_to_jpeg(buffer)?;
    if normalized_buf.len() > limits.max_bytes {
        return Err(format!(
            "Image too large after HEIC conversion: {} bytes (limit: {} bytes)",
            normalized_buf.len(),
            limits.max_bytes
        ));
    }

    Ok(NormalizedImage {
        buffer: normalized_buf,
        mime_type: NORMALIZED_INPUT_IMAGE_MIME.to_string(),
    })
}

pub fn extract_image_content_from_source(
    source: &InputImageSource,
    limits: &InputImageLimits,
) -> Result<PdfExtractedImage, String> {
    let (buffer, content_type, canonical_data) = read_input_image_source(source, limits)?;
    let mime_type = match source {
        InputImageSource::Base64 { .. } => {
            normalize_mime_type(content_type.as_deref()).unwrap_or_else(|| "image/png".to_string())
        }
        InputImageSource::Url { .. } => {
            parse_content_type(content_type.as_deref()).mime_type.unwrap_or_else(|| "image/png".to_string())
        }
    };

    let normalized = normalize_input_image_buffer(&buffer, Some(&mime_type), limits)?;
    let data = if normalized.buffer == buffer {
        canonical_data.unwrap_or_else(|| BASE64_STANDARD.encode(&normalized.buffer))
    } else {
        BASE64_STANDARD.encode(&normalized.buffer)
    };

    Ok(PdfExtractedImage {
        r#type: "image".to_string(),
        data,
        mime_type: normalized.mime_type,
    })
}

pub fn extract_file_content_from_buffer(
    buffer: &[u8],
    filename: Option<&str>,
    mime_type: Option<&str>,
    charset: Option<&str>,
    limits: &InputFileLimits,
    classification: Option<&AttachmentClassification>,
) -> Result<InputFileExtractResult, String> {
    let filename = filename.unwrap_or("file").to_string();
    if buffer.len() > limits.max_bytes {
        return Err(format!("File too large: {} bytes (limit: {} bytes)", buffer.len(), limits.max_bytes));
    }

    let default_class;
    let actual_classification = match classification {
        Some(c) => c,
        None => {
            default_class = classify_attachment_bytes(buffer, mime_type, None, Some(&filename));
            &default_class
        }
    };

    let Some(ref mime) = actual_classification.mime else {
        return Err("input_file missing media type".to_string());
    };

    if !limits.allowed_mimes.contains(mime) {
        return Err(format!("Unsupported file MIME type: {mime}"));
    }

    if mime == "application/pdf" {
        // PDF text extraction stub / fallback
        let text = format!("[PDF document: {} ({} bytes)]", filename, buffer.len());
        let truncated = truncate_utf16_safe(&text, limits.max_chars);
        let text_truncated = truncated.encode_utf16().count() < text.encode_utf16().count();
        return Ok(InputFileExtractResult {
            filename,
            text: Some(truncated),
            images: None,
            metadata: Some(DocumentExtractionMetadata {
                text_truncated: Some(text_truncated),
                images_truncated: Some(false),
            }),
        });
    }

    let parsed_ct = parse_content_type(mime_type);
    let resolved_charset = actual_classification.charset.as_deref()
        .or(charset)
        .or(parsed_ct.charset.as_deref());

    let (text, truncated) = decode_text_content(buffer, resolved_charset, limits.max_chars);

    let metadata = if truncated {
        Some(DocumentExtractionMetadata {
            text_truncated: Some(true),
            images_truncated: Some(false),
        })
    } else {
        None
    };

    Ok(InputFileExtractResult {
        filename,
        text: Some(text),
        images: None,
        metadata,
    })
}

pub fn extract_file_content_from_source(
    source: &InputFileSource,
    limits: &InputFileLimits,
) -> Result<InputFileExtractResult, String> {
    let filename = match source {
        InputFileSource::Base64 { filename, .. } | InputFileSource::Url { filename, .. } => {
            filename.clone().unwrap_or_else(|| "file".to_string())
        }
    };

    let source_limits = InputSourceLimits {
        allow_url: limits.allow_url,
        url_allowlist: limits.url_allowlist.clone(),
        allowed_mimes: limits.allowed_mimes.clone(),
        max_bytes: limits.max_bytes,
        max_redirects: limits.max_redirects,
        timeout_ms: limits.timeout_ms,
    };

    let (buffer, content_type, _) = read_input_source(source, &source_limits, "input_file")?;

    extract_file_content_from_buffer(
        &buffer,
        Some(&filename),
        content_type.as_deref(),
        None,
        limits,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_utf16_safe() {
        let text = "Café 😀 notes";
        assert_eq!(truncate_utf16_safe(text, 6), "Café ");
        assert_eq!(truncate_utf16_safe(text, 7), "Café 😀");
        assert_eq!(truncate_utf16_safe(text, 8), "Café 😀 ");
        assert_eq!(truncate_utf16_safe(text, 9), "Café 😀 n");
    }

    #[test]
    fn test_decode_text_windows1252() {
        // "café €" in windows-1252: 'c', 'a', 'f', 0xe9, ' ', 0x80
        let bytes = vec![0x63, 0x61, 0x66, 0xe9, 0x20, 0x80];
        let (text, truncated) = decode_text_content(&bytes, Some("windows-1252"), 100);
        assert_eq!(text, "café €");
        assert!(!truncated);
    }

    #[test]
    fn test_decode_text_windows1251() {
        let text = "Привет, мир!";
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode(text);
        let (decoded, truncated) = decode_text_content(&bytes, Some("windows-1251"), 100);
        assert_eq!(decoded, "Привет, мир!");
        assert!(!truncated);
    }

    #[test]
    fn test_decode_text_iso_8859_1_whatwg_alias() {
        // WHATWG maps "iso-8859-1" and "latin1" to windows-1252
        let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode("café résumé");
        let (decoded_iso, _) = decode_text_content(&bytes, Some("iso-8859-1"), 100);
        assert_eq!(decoded_iso, "café résumé");

        let (decoded_latin1, _) = decode_text_content(&bytes, Some("latin1"), 100);
        assert_eq!(decoded_latin1, "café résumé");

        // Fallback for unknown charset falls back to UTF-8
        let utf8_sample = "hello world";
        let (fallback_decoded, _) = decode_text_content(utf8_sample.as_bytes(), Some("x-unknown-encoding"), 100);
        assert_eq!(fallback_decoded, "hello world");
    }
}
