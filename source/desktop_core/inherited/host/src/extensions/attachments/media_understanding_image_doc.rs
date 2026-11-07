use serde::{Deserialize, Serialize};

use crate::extensions::attachments::input_files::{
    resolve_input_file_limits, InputFileLimits, InputFileLimitsConfig,
};
use crate::extensions::attachments::media_file_context::{
    render_document_truncation_notice, render_file_context_block, DocumentExtractionMetadata,
    RenderFileContextParams,
};
use super::{MediaAttachmentDisposition, MediaUnderstandingCapability};

pub const INBOUND_FILE_EXTRACTION_DEFAULT_MAX_MB: usize = 20;
pub const INBOUND_FILE_EXTRACTION_MAX_BYTES_CAP: usize = 25 * 1024 * 1024;
pub const INBOUND_FILE_EXTRACTION_DEFAULT_MAX_PAGES: usize = 20;
pub const INBOUND_FILE_EXTRACTION_MAX_PAGES_CAP: usize = 150;
pub const MAX_SKIPPED_FILE_MARKERS: usize = 5;
pub const MARKER_MIME_MAX_CHARS: usize = 100;
pub const MARKER_LOCAL_PATH_MAX_CHARS: usize = 300;
pub const MEDIA_UNDERSTANDING_MAX_SOURCE_PIXELS: usize = 40_000_000;

pub fn append_file_blocks(body: &str, suffix: Option<&str>) -> String {
    let suf = match suffix {
        Some(s) => s.trim(),
        None => return body.to_string(),
    };
    if suf.is_empty() {
        return body.to_string();
    }
    let base = body.trim();
    if base.is_empty() {
        suf.to_string()
    } else {
        format!("{base}\n\n{suf}")
    }
}

pub fn is_media_store_url(url: &str) -> bool {
    let trimmed = url.trim().to_ascii_lowercase();
    trimmed.starts_with("media://")
}

#[derive(Debug, Clone, Default)]
pub struct FileExtractionLimitsConfig {
    pub media_max_mb: Option<usize>,
    pub pdf_max_pages: Option<usize>,
    pub files: Option<InputFileLimitsConfig>,
}

#[derive(Debug, Clone)]
pub struct FileExtractionLimits {
    pub limits: InputFileLimits,
    pub allowed_mimes_configured: bool,
}

pub fn resolve_file_extraction_limits(config: &FileExtractionLimitsConfig) -> FileExtractionLimits {
    let allowed_mimes_configured = config
        .files
        .as_ref()
        .and_then(|f| f.allowed_mimes.as_ref())
        .map_or(false, |m| !m.is_empty());

    let max_mb = config.media_max_mb.unwrap_or(INBOUND_FILE_EXTRACTION_DEFAULT_MAX_MB);
    let max_pages = config.pdf_max_pages.unwrap_or(INBOUND_FILE_EXTRACTION_DEFAULT_MAX_PAGES);

    let mut inbound_files = config.files.clone().unwrap_or_default();
    if inbound_files.max_bytes.is_none() {
        inbound_files.max_bytes = Some(std::cmp::min(
            max_mb * 1024 * 1024,
            INBOUND_FILE_EXTRACTION_MAX_BYTES_CAP,
        ));
    }
    let pdf_cfg = inbound_files.pdf.get_or_insert_with(Default::default);
    if pdf_cfg.max_pages.is_none() {
        pdf_cfg.max_pages = Some(std::cmp::min(
            max_pages,
            INBOUND_FILE_EXTRACTION_MAX_PAGES_CAP,
        ));
    }

    let resolved = resolve_input_file_limits(Some(&inbound_files));
    FileExtractionLimits {
        limits: resolved,
        allowed_mimes_configured,
    }
}

/// Extracted image payload from a document attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentExtractedImage {
    pub r#type: String,
    pub data: String,
    pub mime_type: String,
}

/// Transient image payload extracted from an inbound file attachment with index tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFileImage {
    pub r#type: String,
    pub data: String,
    pub mime_type: String,
    pub attachment_index: usize,
}

pub fn strip_extracted_file_image_metadata(img: ExtractedFileImage) -> DocumentExtractedImage {
    DocumentExtractedImage {
        r#type: "image".to_string(),
        data: img.data,
        mime_type: img.mime_type,
    }
}

pub fn render_skipped_file_overflow_summary(count: usize) -> String {
    format!("[{count} more attachment{} skipped]", if count == 1 { "" } else { "s" })
}

pub fn render_media_attachment_disposition(
    capability: MediaUnderstandingCapability,
    disposition: &MediaAttachmentDisposition,
) -> Option<String> {
    let cap_str = match capability {
        MediaUnderstandingCapability::Audio => "Audio",
        MediaUnderstandingCapability::Image => "Image",
        MediaUnderstandingCapability::Video => "Video",
    };
    let cap_lower = match capability {
        MediaUnderstandingCapability::Audio => "audio",
        MediaUnderstandingCapability::Image => "image",
        MediaUnderstandingCapability::Video => "video",
    };
    match disposition {
        MediaAttachmentDisposition::Handled | MediaAttachmentDisposition::HandedToNativeVision => None,
        MediaAttachmentDisposition::NotSelected => {
            Some(format!("[{cap_str} attachment not processed: attachment limit reached]"))
        }
        MediaAttachmentDisposition::CapabilityDisabled => {
            Some(format!("[{cap_str} attachment not analyzed: {cap_lower} understanding is disabled]"))
        }
        MediaAttachmentDisposition::NoModel => {
            Some(format!("[{cap_str} attachment not analyzed: no {cap_lower}-understanding model is configured]"))
        }
        MediaAttachmentDisposition::ScopeDenied => {
            Some(format!("[{cap_str} attachment not analyzed in this chat]"))
        }
        MediaAttachmentDisposition::Failed { .. } => {
            Some(format!("[{cap_str} attachment could not be analyzed]"))
        }
    }
}

pub struct AttachmentContextBlock {
    pub text: String,
    pub consumes_marker_budget: bool,
}

pub fn apply_attachment_marker_budget(blocks: &[AttachmentContextBlock]) -> Vec<String> {
    let mut rendered = Vec::new();
    let mut markers = 0;
    let mut overflow = 0;
    for block in blocks {
        if block.consumes_marker_budget && markers >= MAX_SKIPPED_FILE_MARKERS {
            overflow += 1;
            continue;
        }
        if block.consumes_marker_budget {
            markers += 1;
        }
        rendered.push(block.text.clone());
    }
    if overflow > 0 {
        rendered.push(render_skipped_file_overflow_summary(overflow));
    }
    rendered
}

static MIME_REGEX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    let mime_type = r"([a-zA-Z0-9!#$&^_.+-]+/[a-zA-Z0-9!#$&^_.+-]+)";
    let http_token = r"[a-zA-Z0-9!#$%&'*+.^_\`|~-]+";
    let http_quoted_string = r#""(?:[\t !#-\[\]-~]|\\[\t -~])*""#;
    let pattern = format!(
        r"(?i)^{mime_type}(?:[ \t]*;[ \t]*{http_token}=(?:{http_token}|{http_quoted_string}))*$"
    );
    regex::Regex::new(&pattern).expect("valid mime regex")
});

pub fn sanitize_mime_type(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let caps = MIME_REGEX.captures(trimmed)?;
    caps.get(1).map(|m| m.as_str().to_ascii_lowercase())
}

pub fn marker_safe_mime(value: Option<&str>) -> Option<String> {
    let mime = sanitize_mime_type(value)?;
    if mime.len() <= MARKER_MIME_MAX_CHARS {
        Some(mime)
    } else {
        None
    }
}

static PATH_SAFE_REGEX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[\p{L}\p{M}\p{N} /\\:._-]+$").expect("valid path safe regex")
});

pub fn marker_safe_local_path(
    value: Option<&str>,
    allow_workspace_relative: bool,
) -> Option<String> {
    let val = value?.trim();
    if val.is_empty() || val.chars().count() > MARKER_LOCAL_PATH_MAX_CHARS {
        return None;
    }
    let is_posix_abs = val.starts_with('/');
    let is_win_abs = val.len() >= 3
        && val.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
        && val.as_bytes()[1] == b':'
        && val.as_bytes()[2] == b'\\';
    let is_absolute = is_posix_abs || is_win_abs;

    if !is_absolute {
        if !allow_workspace_relative || val.contains('\\') {
            return None;
        }
        for segment in val.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return None;
            }
        }
    }

    if PATH_SAFE_REGEX.is_match(val) {
        Some(val.to_string())
    } else {
        None
    }
}

static MARKER_SANITIZED_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"(?i)<<<\s*EXTERNAL[\s_]+UNTRUSTED[\s_]+CONTENT(?:\s+id=\\*"[^"]*")?\s*>>>"#)
        .expect("valid regex")
});

static END_MARKER_SANITIZED_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"(?i)<<<\s*END[\s_]+EXTERNAL[\s_]+UNTRUSTED[\s_]+CONTENT(?:\s+id=\\*"[^"]*")?\s*>>>"#)
        .expect("valid regex")
});

static SPECIAL_TOKENS_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)<\|im_start\|>|<\|im_end\|>|<\|endoftext\|>|<\|begin_of_text\|>|<\|end_of_text\|>|<\|start_header_id\|>|<\|end_header_id\|>|<\|eot_id\|>|<\|python_tag\|>|<\|eom_id\|>|\[INST\]|\[/INST\]|<<SYS>>|<</SYS>>|<s>|</s>|<\|channel\|>|<\|message\|>|<\|return\|>|<\|call\|>|<start_of_turn>|<end_of_turn>|<\|reserved_special_token_\d+\|>")
        .expect("valid regex")
});

pub fn sanitize_external_content_text(content: &str) -> String {
    let step1 = MARKER_SANITIZED_RE.replace_all(content, "[[MARKER_SANITIZED]]");
    let step2 = END_MARKER_SANITIZED_RE.replace_all(&step1, "[[END_MARKER_SANITIZED]]");
    let step3 = SPECIAL_TOKENS_RE.replace_all(&step2, "[REMOVED_SPECIAL_TOKEN]");
    step3.into_owned()
}

pub fn wrap_untrusted_attachment_content(content: &str) -> String {
    let sanitized = sanitize_external_content_text(content);
    let marker_id = &uuid::Uuid::new_v4().simple().to_string()[..16];
    format!("\n<<<EXTERNAL_UNTRUSTED_CONTENT id=\"{marker_id}\">>>\nSource: External\n---\n{sanitized}\n<<<END_EXTERNAL_UNTRUSTED_CONTENT id=\"{marker_id}\">>>")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAttachmentOutcome {
    Extracted {
        text: String,
        images: Vec<DocumentExtractedImage>,
        metadata: Option<DocumentExtractionMetadata>,
    },
    RenderedToImages {
        images: Vec<DocumentExtractedImage>,
        metadata: Option<DocumentExtractionMetadata>,
    },
    NoExtractableText {
        metadata: Option<DocumentExtractionMetadata>,
    },
    UnsupportedFormat {
        mime: Option<String>,
        local_path: Option<String>,
    },
    PolicyRejected {
        mime: Option<String>,
    },
    ReadFailure,
    UrlSourcesDisabled,
    ClaimedElsewhere,
}

pub fn resolve_file_extraction_outcome(
    text: Option<&str>,
    images: Vec<DocumentExtractedImage>,
    metadata: Option<DocumentExtractionMetadata>,
) -> FileAttachmentOutcome {
    let text = text.unwrap_or("");
    let is_text_truncated = metadata.as_ref().map(|m| m.text_truncated).unwrap_or(false);
    if !text.trim().is_empty() || is_text_truncated {
        FileAttachmentOutcome::Extracted {
            text: text.to_string(),
            images,
            metadata,
        }
    } else if !images.is_empty() {
        FileAttachmentOutcome::RenderedToImages { images, metadata }
    } else {
        FileAttachmentOutcome::NoExtractableText { metadata }
    }
}

pub fn is_skipped_file_outcome(outcome: &FileAttachmentOutcome) -> bool {
    matches!(
        outcome,
        FileAttachmentOutcome::UnsupportedFormat { .. }
            | FileAttachmentOutcome::PolicyRejected { .. }
            | FileAttachmentOutcome::ReadFailure
            | FileAttachmentOutcome::UrlSourcesDisabled
    )
}

#[derive(Debug, Clone, Default)]
pub struct RenderFileAttachmentOutcomeOptions<'a> {
    pub self_serve_local_path: Option<&'a str>,
    pub disable_self_serve: bool,
}

pub fn render_file_attachment_outcome(
    outcome: &FileAttachmentOutcome,
    options: Option<&RenderFileAttachmentOutcomeOptions>,
) -> Option<String> {
    match outcome {
        FileAttachmentOutcome::Extracted { text, metadata, .. } => {
            let notice = render_document_truncation_notice(metadata.as_ref(), None);
            let wrapped = wrap_untrusted_attachment_content(text);
            if let Some(n) = notice {
                Some(format!("{n}\n{wrapped}"))
            } else {
                Some(wrapped)
            }
        }
        FileAttachmentOutcome::RenderedToImages { metadata, .. } => {
            let notice = render_document_truncation_notice(metadata.as_ref(), None);
            let marker = "[PDF content rendered to images]";
            if let Some(n) = notice {
                Some(format!("{n}\n{marker}"))
            } else {
                Some(marker.to_string())
            }
        }
        FileAttachmentOutcome::NoExtractableText { metadata } => {
            let notice = render_document_truncation_notice(metadata.as_ref(), None);
            let marker = "[No extractable text]";
            if let Some(n) = notice {
                Some(format!("{n}\n{marker}"))
            } else {
                Some(marker.to_string())
            }
        }
        FileAttachmentOutcome::UnsupportedFormat { mime, local_path } => {
            let safe_mime = marker_safe_mime(mime.as_deref());
            let format_clause = match &safe_mime {
                Some(m) => format!("Unsupported document format: {m}."),
                None => "Unsupported document format.".to_string(),
            };
            let (target_path, allow_rel) = if let Some(opts) = options {
                if opts.disable_self_serve {
                    (None, false)
                } else if let Some(override_path) = opts.self_serve_local_path {
                    (Some(override_path), true)
                } else {
                    (local_path.as_deref(), false)
                }
            } else {
                (local_path.as_deref(), false)
            };
            let validated_path = marker_safe_local_path(target_path, allow_rel);
            let format_hint = if mime
                .as_deref()
                .map_or(false, |m| m.starts_with("application/vnd.openxmlformats-officedocument"))
            {
                " (this Office file is a zip archive containing XML)"
            } else {
                ""
            };
            match validated_path {
                Some(path) => {
                    let guidance = format!(
                        "[{format_clause} The approved local file path follows as external attachment metadata. Its text is not extracted automatically. Read the file yourself with your tools before answering{format_hint}; do not ask the user to paste the contents.]"
                    );
                    let wrapped = wrap_untrusted_attachment_content(&path);
                    Some(format!("{guidance}{wrapped}"))
                }
                None => Some(format!(
                    "[{format_clause} PDF and plain-text attachments can be read.]"
                )),
            }
        }
        FileAttachmentOutcome::PolicyRejected { mime } => {
            let safe_mime = marker_safe_mime(mime.as_deref());
            match safe_mime {
                Some(m) => Some(format!("[Attachment type not allowed: {m}]")),
                None => Some("[Attachment type not allowed]".to_string()),
            }
        }
        FileAttachmentOutcome::ReadFailure => Some("[Attachment could not be read]".to_string()),
        FileAttachmentOutcome::UrlSourcesDisabled => {
            Some("[Attachment skipped: URL file sources are disabled]".to_string())
        }
        FileAttachmentOutcome::ClaimedElsewhere => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundDocumentContext {
    pub text: String,
    pub images: Vec<ExtractedFileImage>,
}

#[derive(Debug, Clone)]
pub struct InboundDocumentItem {
    pub index: usize,
    pub file_name: Option<String>,
    pub mime: Option<String>,
    pub local_path: Option<String>,
    pub outcome: FileAttachmentOutcome,
}

pub fn render_inbound_document_context_from_items(
    items: &[InboundDocumentItem],
    self_serve_paths_enabled: bool,
) -> InboundDocumentContext {
    let mut blocks: Vec<AttachmentContextBlock> = Vec::new();
    let mut images: Vec<ExtractedFileImage> = Vec::new();

    for item in items {
        if let FileAttachmentOutcome::Extracted { images: imgs, .. }
        | FileAttachmentOutcome::RenderedToImages { images: imgs, .. } = &item.outcome
        {
            for img in imgs {
                images.push(ExtractedFileImage {
                    r#type: "image".to_string(),
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                    attachment_index: item.index,
                });
            }
        }

        let options = RenderFileAttachmentOutcomeOptions {
            self_serve_local_path: None,
            disable_self_serve: !self_serve_paths_enabled,
        };
        let block_text = render_file_attachment_outcome(&item.outcome, Some(&options));
        if let Some(content) = block_text {
            let fallback = format!("file-{}", item.index + 1);
            let rendered_block = render_file_context_block(RenderFileContextParams {
                filename: item.file_name.as_deref(),
                fallback_name: Some(&fallback),
                mime_type: item.mime.as_deref(),
                content: &content,
                surround_content_with_newlines: true,
            });
            let consumes = is_skipped_file_outcome(&item.outcome);
            blocks.push(AttachmentContextBlock {
                text: rendered_block,
                consumes_marker_budget: consumes,
            });
        }
    }

    let budgeted = apply_attachment_marker_budget(&blocks);
    InboundDocumentContext {
        text: budgeted.join("\n\n"),
        images,
    }
}

pub fn is_heic_image_input(mime: Option<&str>, file_name: Option<&str>) -> bool {
    if let Some(m) = mime.map(str::trim).filter(|s| !s.is_empty()) {
        let lower = m.to_ascii_lowercase();
        if lower == "image/heic"
            || lower == "image/heif"
            || lower == "image/heic-sequence"
            || lower == "image/heif-sequence"
        {
            return true;
        }
    }
    if let Some(f) = file_name.map(str::trim).filter(|s| !s.is_empty()) {
        let lower = f.to_ascii_lowercase();
        if lower.ends_with(".heic") || lower.ends_with(".heif") {
            return true;
        }
    }
    false
}

pub fn effective_image_bytes_cap(max_bytes: usize, model_max_bytes: Option<usize>) -> usize {
    model_max_bytes
        .filter(|&m| m > 0)
        .map_or(max_bytes, |m| std::cmp::min(max_bytes, m))
}

pub fn require_image_capable_model(
    model_provider: &str,
    model_id: &str,
    input_capabilities: &[String],
) -> Result<(), String> {
    if input_capabilities.iter().any(|c| c == "image") {
        Ok(())
    } else {
        let caps = if input_capabilities.is_empty() {
            "none".to_string()
        } else {
            input_capabilities.join(", ")
        };
        Err(format!(
            "Model does not support images: {model_provider}/{model_id} (input: {caps})"
        ))
    }
}

pub fn resolve_image_tool_max_tokens(
    model_max_tokens: Option<usize>,
    requested_max_tokens: usize,
) -> usize {
    match model_max_tokens {
        Some(m) if m > 0 => std::cmp::min(m, requested_max_tokens),
        _ => requested_max_tokens,
    }
}

pub fn disable_reasoning_for_image_retry_payload(
    payload: &serde_json::Value,
    is_native_responses: bool,
) -> Option<serde_json::Value> {
    let mut map = payload.as_object()?.clone();
    map.remove("reasoning");
    map.remove("reasoning_effort");

    if let Some(include) = map.get("include").and_then(|v| v.as_array()) {
        let filtered: Vec<serde_json::Value> = include
            .iter()
            .filter(|v| v.as_str() != Some("reasoning.encrypted_content"))
            .cloned()
            .collect();
        if filtered.is_empty() {
            map.remove("include");
        } else {
            map.insert("include".to_string(), serde_json::Value::Array(filtered));
        }
    }

    if is_native_responses {
        map.insert(
            "reasoning".to_string(),
            serde_json::json!({ "effort": "none" }),
        );
    }

    Some(serde_json::Value::Object(map))
}

pub fn should_place_image_prompt_in_user_content(provider: &str) -> bool {
    provider.trim().eq_ignore_ascii_case("github-copilot")
}

pub fn is_image_model_no_text_error(message: &str) -> bool {
    message.trim_start().starts_with("Image model returned no text")
}
