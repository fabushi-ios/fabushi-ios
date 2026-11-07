//! Host read media policy and format admission guards.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/host-read-media-policy.ts`

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::media_mime::{
    audio_mime_from_path, image_mime_from_path, video_mime_from_path,
};

pub const HOST_READ_DECLARED_TEXT_ERROR: &str =
    "hostReadCapability permits only validated plain-text documents \
    and validated HTML reports with generated provenance or explicit publication for local reads";

static HOST_READ_TEXT_PLAIN_ALIASES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("text/csv");
    s.insert("text/markdown");
    s.insert("text/plain");
    s.insert("application/json");
    s.insert("application/yaml");
    s
});

static HOST_READ_ALLOWED_DOCUMENT_MIMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("application/msword");
    s.insert("application/pdf");
    s.insert("application/vnd.ms-excel");
    s.insert("application/vnd.ms-excel.sheet.macroenabled.12");
    s.insert("application/vnd.ms-powerpoint");
    s.insert("application/vnd.openxmlformats-officedocument.presentationml.presentation");
    s.insert("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
    s.insert("application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    s.insert("application/gzip");
    s.insert("application/x-7z-compressed");
    s.insert("application/x-tar");
    s.insert("application/zip");
    for alias in HOST_READ_TEXT_PLAIN_ALIASES.iter() {
        s.insert(*alias);
    }
    s
});

static HOST_READ_DECLARED_TEXT_MIMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    for alias in HOST_READ_TEXT_PLAIN_ALIASES.iter() {
        s.insert(*alias);
    }
    s.insert("text/html");
    s
});

static HTML_SHAPE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:<!doctype\s+html|<html)|</(?:html|body)>").expect("valid regex")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReadHtmlTrust {
    TempRoot,
    Outbound {
        expected_sha256: String,
        expected_size: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostReadMediaTypeError(pub String);

impl std::fmt::Display for HostReadMediaTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostReadMediaTypeError: {}", self.0)
    }
}

impl std::error::Error for HostReadMediaTypeError {}

/// Computes the ratio of printable characters in a string.
pub fn text_printable_ratio(text: &str) -> f64 {
    let mut printable = 0usize;
    let mut total = 0usize;
    for c in text.chars() {
        total += 1;
        let code = c as u32;
        if code == 9 || code == 10 || code == 13 || (code >= 32 && (code < 0x7f || code > 0x9f)) {
            printable += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        printable as f64 / total as f64
    }
}

/// Checks if a buffer matches single-byte ASCII/Latin-1 text shape without control bytes.
pub fn has_single_byte_text_shape(buffer: &[u8]) -> bool {
    if buffer.is_empty() {
        return true;
    }
    let mut ascii_text = 0usize;
    let mut control = 0usize;
    for &byte in buffer {
        if byte == 9 || byte == 10 || byte == 13 || (byte >= 0x20 && byte <= 0x7e) {
            ascii_text += 1;
            continue;
        }
        if byte < 0x20 || byte == 0x7f {
            control += 1;
        }
    }
    let total = buffer.len();
    let high_bytes = total.saturating_sub(ascii_text).saturating_sub(control);
    control == 0 && (ascii_text as f64 / total as f64 >= 0.7) && (high_bytes as f64 / total as f64 <= 0.3)
}

/// Decodes host read text using strict UTF-8, falling back to Latin-1 only if single-byte text shape matches.
pub fn decode_host_read_text(buffer: &[u8]) -> Option<String> {
    if let Ok(utf8_str) = std::str::from_utf8(buffer) {
        return Some(utf8_str.to_string());
    }
    if !has_single_byte_text_shape(buffer) {
        return None;
    }
    // Latin-1 (ISO-8859-1) maps byte directly to Unicode scalar value
    Some(buffer.iter().map(|&b| b as char).collect())
}

/// Validates decoded text, requiring printable ratio > 0.95.
pub fn get_validated_host_read_text(buffer: Option<&[u8]>) -> Option<String> {
    let buf = buffer?;
    if buf.is_empty() {
        return Some(String::new());
    }
    let text = decode_host_read_text(buf)?;
    if text_printable_ratio(&text) > 0.95 {
        Some(text)
    } else {
        None
    }
}

/// Checks if text looks like an HTML document header or structure.
pub fn has_html_document_shape(text: &str) -> bool {
    let trimmed = text.trim_start();
    let sample = if trimmed.len() > 8192 {
        &trimmed[..8192]
    } else {
        trimmed
    };
    HTML_SHAPE_REGEX.is_match(sample)
}

fn get_file_extension(path: Option<&Path>) -> Option<String> {
    let p = path?;
    let ext = p.extension()?.to_str()?;
    Some(format!(".{}", ext.to_ascii_lowercase()))
}

fn mime_from_file_path(path: Option<&Path>) -> Option<String> {
    let p = path?;
    if let Some(img) = image_mime_from_path(p) {
        return Some(img.to_string());
    }
    if let Some(vid) = video_mime_from_path(p) {
        return Some(vid.to_string());
    }
    if let Some(aud) = audio_mime_from_path(p) {
        return Some(aud.to_string());
    }
    let ext = get_file_extension(Some(p))?;
    match ext.as_str() {
        ".pdf" => Some("application/pdf".to_string()),
        ".doc" => Some("application/msword".to_string()),
        ".docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()),
        ".xls" => Some("application/vnd.ms-excel".to_string()),
        ".xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()),
        ".ppt" => Some("application/vnd.ms-powerpoint".to_string()),
        ".pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string()),
        ".zip" => Some("application/zip".to_string()),
        ".tar" => Some("application/x-tar".to_string()),
        ".gz" => Some("application/gzip".to_string()),
        ".7z" => Some("application/x-7z-compressed".to_string()),
        ".txt" => Some("text/plain".to_string()),
        ".csv" => Some("text/csv".to_string()),
        ".md" => Some("text/markdown".to_string()),
        ".html" | ".htm" => Some("text/html".to_string()),
        ".json" => Some("application/json".to_string()),
        ".yaml" | ".yml" => Some("application/yaml".to_string()),
        _ => None,
    }
}

pub fn is_allowed_host_read_html(
    sniffed_content_type: Option<&str>,
    buffer: Option<&[u8]>,
    trusted_generated_html_path: Option<&HostReadHtmlTrust>,
    explicit_html_publication: bool,
) -> bool {
    let sniffed_mime = sniffed_content_type.map(|s| s.trim().to_ascii_lowercase());
    if let Some(ref mime) = sniffed_mime {
        if mime != "text/html" {
            return false;
        }
    }
    if trusted_generated_html_path.is_none() && !explicit_html_publication {
        return false;
    }
    let text = match get_validated_host_read_text(buffer) {
        Some(t) => t,
        None => return false,
    };
    if !has_html_document_shape(&text) {
        return false;
    }
    if explicit_html_publication {
        return true;
    }
    match trusted_generated_html_path {
        Some(HostReadHtmlTrust::TempRoot) => true,
        Some(HostReadHtmlTrust::Outbound { expected_sha256, expected_size }) => {
            let buf = match buffer {
                Some(b) => b,
                None => return false,
            };
            if buf.len() != *expected_size {
                return false;
            }
            let actual_sha = format!("{:x}", Sha256::digest(buf));
            actual_sha.eq_ignore_ascii_case(expected_sha256)
        }
        None => false,
    }
}

pub fn is_allowed_host_read_text_alias(mime: Option<&str>, file_path: Option<&Path>) -> bool {
    let m = match mime {
        Some(s) => s.trim().to_ascii_lowercase(),
        None => return false,
    };
    if !HOST_READ_TEXT_PLAIN_ALIASES.contains(m.as_str()) {
        return false;
    }
    if m == "text/plain" {
        get_file_extension(file_path).as_deref() == Some(".txt")
    } else {
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct AssertHostReadParams<'a> {
    pub sniffed_content_type: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub file_path: Option<&'a Path>,
    pub kind: Option<&'a str>,
    pub buffer: Option<&'a [u8]>,
    pub trusted_generated_html_path: Option<&'a HostReadHtmlTrust>,
    pub explicit_html_publication: bool,
}

pub fn assert_host_read_media_allowed(params: &AssertHostReadParams<'_>) -> Result<(), HostReadMediaTypeError> {
    let declared_mime = mime_from_file_path(params.file_path);
    let normalized_mime = params.content_type.map(|s| s.trim().to_ascii_lowercase());

    if let Some(ref declared) = declared_mime {
        if HOST_READ_DECLARED_TEXT_MIMES.contains(declared.as_str()) {
            if declared == "text/html"
                && is_allowed_host_read_html(
                    params.sniffed_content_type,
                    params.buffer,
                    params.trusted_generated_html_path,
                    params.explicit_html_publication,
                )
            {
                return Ok(());
            }
            if is_allowed_host_read_text_alias(Some(declared.as_str()), params.file_path)
                && params.sniffed_content_type.is_none()
                && params.buffer.is_some()
                && get_validated_host_read_text(params.buffer).is_some()
            {
                return Ok(());
            }
            return Err(HostReadMediaTypeError(HOST_READ_DECLARED_TEXT_ERROR.to_string()));
        }
    }

    let sniffed_mime = params.sniffed_content_type.map(|s| s.trim().to_ascii_lowercase());
    if let Some(ref sniffed) = sniffed_mime {
        if sniffed.starts_with("image/") || sniffed.starts_with("audio/") || sniffed.starts_with("video/") {
            return Ok(());
        }
        if HOST_READ_ALLOWED_DOCUMENT_MIMES.contains(sniffed.as_str()) {
            return Ok(());
        }
        if sniffed == "application/x-cfb" {
            let ext = get_file_extension(params.file_path).unwrap_or_default();
            if ext == ".doc" || ext == ".ppt" || ext == ".xls" {
                return Ok(());
            }
        }
    }

    if sniffed_mime.is_none() {
        if let Some(ref norm) = normalized_mime {
            if is_allowed_host_read_text_alias(Some(norm.as_str()), params.file_path)
                && params.buffer.is_some()
                && get_validated_host_read_text(params.buffer).is_some()
            {
                return Ok(());
            }
        }
    }

    if params.kind == Some("document") {
        if let Some(ref norm) = normalized_mime {
            if HOST_READ_ALLOWED_DOCUMENT_MIMES.contains(norm.as_str()) {
                return Err(HostReadMediaTypeError(format!(
                    "Host-local media sends require buffer-verified media/document types (got fallback {norm})."
                )));
            }
        }
    }

    let got_type = sniffed_mime
        .as_deref()
        .or(normalized_mime.as_deref())
        .unwrap_or("unknown");
    Err(HostReadMediaTypeError(format!(
        "Host-local media sends only allow buffer-verified images, audio, video, PDF, Office documents, archives, and validated plain-text documents (got {got_type})."
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedHtmlProvenanceRow {
    pub realpath: String,
    pub kind: String,
    pub version: i64,
    pub sha256: String,
    pub size_bytes: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlProvenanceMarker {
    pub sha256: String,
    pub size: usize,
}

pub fn validate_provenance_row(row: &GeneratedHtmlProvenanceRow) -> Option<HtmlProvenanceMarker> {
    if row.kind == "trusted-generated-html" && row.version == 1 {
        Some(HtmlProvenanceMarker {
            sha256: row.sha256.clone(),
            size: row.size_bytes.max(0) as usize,
        })
    } else {
        None
    }
}

pub fn filter_stale_provenance_rows(rows: &[GeneratedHtmlProvenanceRow]) -> Vec<GeneratedHtmlProvenanceRow> {
    let mut stale = Vec::new();
    for row in rows {
        let path = Path::new(&row.realpath);
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => {
                stale.push(row.clone());
                continue;
            }
        };
        if !meta.is_file() || meta.file_type().is_symlink() {
            stale.push(row.clone());
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                stale.push(row.clone());
            }
        }
    }
    stale
}

