//! Media file context formatting, local path resolution, temp cleanup, and image processing contracts.
//!
//! Maps upstream OpenClaw media modules:
//! - `src/media/file-context.ts`
//! - `src/media/local-media-path.ts`
//! - `src/media/temp-files.ts`
//! - `src/media/document-extraction-metadata.ts`
//! - `src/media/runtime-prompt-image-provenance.ts`
//! - `src/media/image-optimization-error.ts`
//! - `src/media/image-processor-config.ts`
//! - `src/media/image-processor.types.ts`
//! - `src/media/media-services.ts`

use std::path::{Path, PathBuf};
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

/// Shared input/output pixel cap for Rastermill/image processing operations.
pub const MAX_IMAGE_INPUT_PIXELS: usize = 25_000_000;

/// Image processor pixel boundary limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageProcessorPixelLimits {
    pub input_pixels: usize,
    pub output_pixels: usize,
}

impl Default for ImageProcessorPixelLimits {
    fn default() -> Self {
        Self {
            input_pixels: MAX_IMAGE_INPUT_PIXELS,
            output_pixels: MAX_IMAGE_INPUT_PIXELS,
        }
    }
}

/// Image optimization error when an image cannot satisfy its byte budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageOptimizationLimitError {
    pub message: String,
    pub max_bytes: usize,
}

impl std::fmt::Display for ImageOptimizationLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ImageOptimizationLimitError: {} (max_bytes={})",
            self.message, self.max_bytes
        )
    }
}

impl std::error::Error for ImageOptimizationLimitError {}

/// Image operations supported by the local image processor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ImageProcessorOperation {
    #[serde(rename = "encode")]
    Encode { quality: Option<u8> },
    #[serde(rename = "transparency")]
    Transparency,
    #[serde(rename = "bmpToPng")]
    BmpToPng,
}

/// Request for image processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageProcessorRequest {
    pub input_bytes: Vec<u8>,
    pub limits: Option<ImageProcessorPixelLimits>,
    pub operation: ImageProcessorOperation,
}

/// XML attribute value escaping helper (&, <, >, ", ').
pub fn escape_xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Sanitizes an untrusted filename, stripping directory separators, injection quotes, and brackets.
pub fn sanitize_untrusted_file_name(name: &str, fallback: &str) -> String {
    let stripped: String = name
        .chars()
        .filter(|&c| {
            c != '<'
                && c != '>'
                && c != '"'
                && c != '/'
                && c != '\\'
                && c != '\0'
                && !c.is_control()
        })
        .collect();
    let trimmed = stripped.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parameters for rendering a model-visible file context block.
pub struct RenderFileContextParams<'a> {
    pub filename: Option<&'a str>,
    pub fallback_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub content: &'a str,
    pub surround_content_with_newlines: bool,
}

/// Renders sanitized attachment text as a model-visible file block without allowing file-tag injection.
pub fn render_file_context_block(params: RenderFileContextParams<'_>) -> String {
    let fallback = params
        .fallback_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("attachment");

    let normalized_name = params
        .filename
        .map(|name| {
            let re_ws = Regex::new(r"[\r\n\t]+").expect("valid regex");
            re_ws.replace_all(name, " ").into_owned()
        })
        .unwrap_or_default();

    let safe_name = sanitize_untrusted_file_name(&normalized_name, fallback);

    let re_closing = Regex::new(r"(?i)<\s*/\s*file\s*>").expect("valid regex");
    let re_opening = Regex::new(r"(?i)<\s*file\b").expect("valid regex");

    let content_after_closing = re_closing.replace_all(params.content, "&lt;/file&gt;");
    let safe_content = re_opening.replace_all(&content_after_closing, "&lt;file");

    let mut attrs = format!("name=\"{}\"", escape_xml_attr(&safe_name));
    if let Some(mime) = params.mime_type.map(str::trim).filter(|s| !s.is_empty()) {
        attrs.push_str(&format!(" mime=\"{}\"", escape_xml_attr(mime)));
    }

    if params.surround_content_with_newlines {
        format!("<file {attrs}>\n{safe_content}\n</file>")
    } else {
        format!("<file {attrs}>{safe_content}</file>")
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Resolves a media source to a local filesystem path when it is not a remote URL or data URL.
pub fn resolve_local_media_path(source: &str) -> Option<PathBuf> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("data:")
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("media:")
        || lower.starts_with("blob:")
    {
        return None;
    }

    if lower.starts_with("file:") {
        if lower.contains("%2f") || lower.contains("%5c") {
            return None;
        }

        let mut url_str = trimmed;
        let mut single_slash_buf = String::new();
        if !lower.starts_with("file://") && lower.starts_with("file:") {
            let rest = &trimmed[5..];
            let trimmed_rest = rest.trim_start_matches('/');
            single_slash_buf = format!("file:///{}", trimmed_rest);
            url_str = &single_slash_buf;
        }

        let parsed = Url::parse(url_str).ok()?;
        if let Some(host) = parsed.host_str() {
            if !host.is_empty() && host != "localhost" {
                return None;
            }
        }
        return parsed.to_file_path().ok();
    }

    if trimmed.starts_with('~') {
        if trimmed == "~" {
            return home_dir();
        }
        if trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
            let sub = &trimmed[2..];
            return home_dir().map(|h| h.join(sub));
        }
    }

    let p = Path::new(trimmed);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }

    let is_windows_drive = trimmed.len() >= 3
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':'
        && (trimmed.as_bytes()[2] == b'/' || trimmed.as_bytes()[2] == b'\\');
    if is_windows_drive {
        return Some(PathBuf::from(trimmed));
    }

    None
}

/// Best-effort temp file cleanup helper.
pub fn unlink_if_exists(file_path: Option<&Path>) {
    if let Some(path) = file_path {
        let _ = std::fs::remove_file(path);
    }
}

/// Page-level extraction status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPagesMetadata {
    pub processed: Vec<usize>,
    pub total: usize,
    pub truncated: bool,
    pub selection: String, // "automatic" | "explicit"
}

/// Metadata describing partial extraction or truncation of document attachments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentExtractionMetadata {
    pub pages: Option<DocumentPagesMetadata>,
    pub text_truncated: bool,
    pub images_truncated: bool,
}

/// Renders a human-readable notice for partial or truncated document extraction.
pub fn render_document_truncation_notice(
    metadata: Option<&DocumentExtractionMetadata>,
    explicit_selection_limit: Option<usize>,
) -> Option<String> {
    let mut reasons = Vec::new();

    if let Some(limit) = explicit_selection_limit {
        reasons.push(format!("requested page selection limited to {} pages", limit));
    }

    if let Some(meta) = metadata {
        if let Some(ref pages) = meta.pages {
            if pages.truncated {
                if pages.selection == "automatic" {
                    reasons.push(format!(
                        "{} of {} pages processed",
                        pages.processed.len(),
                        pages.total
                    ));
                } else {
                    reasons.push(format!(
                        "some requested pages omitted; {} pages processed",
                        pages.processed.len()
                    ));
                }
            }
        }
        if meta.text_truncated {
            reasons.push("text truncated".to_string());
        }
        if meta.images_truncated {
            reasons.push("image rendering truncated".to_string());
        }
    }

    if reasons.is_empty() {
        None
    } else {
        Some(format!("[Partial document: {}.]", reasons.join("; ")))
    }
}

/// Runtime prompt image entry with fact provenance tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePromptImageEntry {
    pub image_url: String,
    pub fact_index: Option<usize>,
}

/// Finalized bundle of prompt images with fact index provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizedPromptImages {
    pub images: Vec<String>,
    pub fact_indexes: Vec<Option<usize>>,
}

/// Finalizes runtime prompt image entries, preserving fact ownership.
pub fn finalize_runtime_prompt_images(entries: &[RuntimePromptImageEntry]) -> FinalizedPromptImages {
    FinalizedPromptImages {
        images: entries.iter().map(|e| e.image_url.clone()).collect(),
        fact_indexes: entries.iter().map(|e| e.fact_index).collect(),
    }
}

/// Reads prompt image fact indexes from a finalized bundle.
pub fn read_runtime_prompt_image_fact_indexes(
    bundle: &FinalizedPromptImages,
) -> Option<Vec<Option<usize>>> {
    if bundle.images.is_empty() {
        None
    } else {
        Some(bundle.fact_indexes.clone())
    }
}
