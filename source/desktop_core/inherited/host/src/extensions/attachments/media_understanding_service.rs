use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

#[path = "media_understanding_audio.rs"]
pub mod media_understanding_audio;
#[path = "media_understanding_image_doc.rs"]
pub mod media_understanding_image_doc;
#[path = "media_understanding_providers.rs"]
pub mod media_understanding_providers;

// Re-export submodules
pub use self::media_understanding_audio::{
    build_audio_transcription_form_data, build_open_ai_compatible_auth_headers, can_deliver_transcript_echo,
    command_id, extract_sherpa_onnx_text, format_echo_transcript, format_local_audio_selection,
    is_deliverable_message_channel, is_transcript_artifact_text, rank_local_audio_candidate,
    record_local_audio_backend_observation, resolve_audio_input_budget_for_config,
    resolve_audio_transcription_upload_file_name, resolve_openai_audio_auth_model_api,
    resolve_parakeet_output_path, resolve_requested_local_audio_backend, resolve_whisper_cpp_output_path,
    resolve_whisper_output_path, AudioInputBudget, AudioTranscriptionFormData, LocalAudioCandidate,
    LocalAudioSelection, DEFAULT_ECHO_TRANSCRIPT_FORMAT, EMPTY_VOICE_NOTE_PLACEHOLDER,
    MIN_AUDIO_FILE_BYTES, OPENAI_AUDIO_TRANSCRIPTIONS_API,
};

pub use self::media_understanding_image_doc::{
    append_file_blocks, apply_attachment_marker_budget, disable_reasoning_for_image_retry_payload,
    effective_image_bytes_cap, is_heic_image_input, is_image_model_no_text_error, is_media_store_url,
    is_skipped_file_outcome, marker_safe_local_path, marker_safe_mime, render_file_attachment_outcome,
    render_inbound_document_context_from_items, render_media_attachment_disposition,
    render_skipped_file_overflow_summary, require_image_capable_model, resolve_file_extraction_limits,
    resolve_file_extraction_outcome, resolve_image_tool_max_tokens, sanitize_external_content_text,
    sanitize_mime_type, should_place_image_prompt_in_user_content, strip_extracted_file_image_metadata,
    wrap_untrusted_attachment_content, AttachmentContextBlock, DocumentExtractedImage, ExtractedFileImage,
    FileAttachmentOutcome, FileExtractionLimits, FileExtractionLimitsConfig, InboundDocumentContext,
    InboundDocumentItem, RenderFileAttachmentOutcomeOptions, INBOUND_FILE_EXTRACTION_DEFAULT_MAX_MB,
    INBOUND_FILE_EXTRACTION_DEFAULT_MAX_PAGES, INBOUND_FILE_EXTRACTION_MAX_BYTES_CAP,
    INBOUND_FILE_EXTRACTION_MAX_PAGES_CAP, MARKER_LOCAL_PATH_MAX_CHARS, MARKER_MIME_MAX_CHARS,
    MAX_SKIPPED_FILE_MARKERS, MEDIA_UNDERSTANDING_MAX_SOURCE_PIXELS,
};

pub use self::media_understanding_providers::{
    build_media_understanding_registry, build_openai_compatible_video_request_body,
    coerce_openai_compatible_video_text, format_decision_summary, is_native_media_deliverable,
    matches_media_entry_capability, normalize_decision_reason, normalize_media_attachments_with_sticker_rule,
    normalize_media_provider_id, preferred_media_model_rank, resolve_auto_media_key_providers_from_registry,
    resolve_cli_model_entry, resolve_configured_media_entry_capabilities,
    resolve_default_media_model_from_registry, resolve_effective_media_entry_capabilities,
    resolve_entry_run_prompt, resolve_media_runtime_timeout_ms, resolve_model_entry_secret_owner_id,
    resolve_provider_request_context, resolve_scope_decision, resolve_timeout_ms,
    summarize_decision_reason, MediaCliModelUnavailableError, MediaCliModelUnavailableReason,
    MediaScopeRule, MediaScopeRuleMatch, MediaUnderstandingModelConfig, MediaUnderstandingProvider,
    MediaUnderstandingProviderEntry, MediaUnderstandingScopeConfig, ScopeContext, CLI_OUTPUT_MAX_BUFFER,
    DEFAULT_MEDIA_CONCURRENCY, DEFAULT_MEDIA_RUNTIME_TIMEOUT_MS, MAX_TIMER_TIMEOUT_MS, MIN_MEDIA_TIMEOUT_MS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaUnderstandingCapability {
    Image,
    Audio,
    Video,
}

impl MediaUnderstandingCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

pub const CAPABILITY_ORDER: &[MediaUnderstandingCapability] = &[
    MediaUnderstandingCapability::Image,
    MediaUnderstandingCapability::Audio,
    MediaUnderstandingCapability::Video,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaUnderstandingDecisionOutcome {
    Success,
    Failed,
    Skipped,
    Disabled,
    NoAttachment,
    ScopeDeny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MediaAttachmentDisposition {
    Handled,
    HandedToNativeVision,
    NotSelected,
    CapabilityDisabled,
    NoModel,
    ScopeDenied,
    Failed { reason: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaAttachmentProcessing {
    Completed,
    Omitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUnderstandingModelDecision {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub requested_backend: Option<String>,
    pub observed_backend: Option<String>,
    pub decision_type: String, // "provider" or "cli"
    pub outcome: String,       // "success", "skipped", "failed"
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUnderstandingDecision {
    pub capability: MediaUnderstandingCapability,
    pub outcome: MediaUnderstandingDecisionOutcome,
    pub attachment_dispositions: HashMap<usize, MediaAttachmentDisposition>,
    pub attachment_processing: HashMap<usize, MediaAttachmentProcessing>,
    pub native_vision_active: bool,
}

pub fn resolve_media_understanding_string(value: Option<&str>, fallback: &str) -> String {
    let trimmed = value.map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedMediaAttachment {
    pub path: Option<String>,
    pub url: Option<String>,
    pub mime: Option<String>,
    pub kind: Option<String>,
    pub file_name: Option<String>,
    pub workspace_dir: Option<String>,
    pub index: usize,
    pub already_transcribed: bool,
}

/// Normalizes local attachment paths while rejecting Windows UNC network paths and remote file schemes.
pub fn normalize_attachment_path(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    // Reject Windows UNC network paths (e.g. \\server\share or //server/share)
    if raw.starts_with(r"\\") || raw.starts_with("//") {
        return None;
    }
    if raw.to_ascii_lowercase().starts_with("file://") {
        let stripped = &raw[7..];
        if stripped.starts_with(r"\\") || stripped.starts_with("//") {
            return None;
        }
        return Some(stripped.to_string());
    }
    Some(raw.to_string())
}

/// Classifies an attachment by authoritative kind, MIME, then filename extension.
pub fn resolve_attachment_media_kind(attachment: &NormalizedMediaAttachment) -> &'static str {
    if let Some(ref k) = attachment.kind {
        let lk = k.to_ascii_lowercase();
        match lk.as_str() {
            "image" | "sticker" => return "image",
            "audio" => return "audio",
            "video" => return "video",
            "document" => return "unknown",
            _ => {}
        }
    }
    if let Some(ref m) = attachment.mime {
        let lm = m.to_ascii_lowercase();
        if lm.starts_with("image/") {
            return "image";
        }
        if lm.starts_with("audio/") {
            return "audio";
        }
        if lm.starts_with("video/") {
            return "video";
        }
    }
    if let Some(ref p) = attachment.path.as_deref().or(attachment.file_name.as_deref()) {
        let path = Path::new(p);
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            let lext = ext.to_ascii_lowercase();
            match lext.as_str() {
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" => return "image",
                "mp3" | "wav" | "m4a" | "aac" | "ogg" | "flac" => return "audio",
                "mp4" | "mov" | "webm" | "mkv" | "avi" => return "video",
                _ => {}
            }
        }
    }
    "unknown"
}

pub fn is_audio_attachment(attachment: &NormalizedMediaAttachment) -> bool {
    resolve_attachment_media_kind(attachment) == "audio"
}

pub fn is_image_attachment(attachment: &NormalizedMediaAttachment) -> bool {
    resolve_attachment_media_kind(attachment) == "image"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentOrderPreference {
    First,
    Last,
    Path,
    Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentSelectionMode {
    First,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSelectionPolicy {
    pub prefer: Option<AttachmentOrderPreference>,
    pub mode: AttachmentSelectionMode,
    pub max_attachments: usize,
}

impl Default for AttachmentSelectionPolicy {
    fn default() -> Self {
        Self {
            prefer: None,
            mode: AttachmentSelectionMode::First,
            max_attachments: 1,
        }
    }
}

pub struct AttachmentSelectionResult {
    pub selected: Vec<NormalizedMediaAttachment>,
    pub dropped_attachment_indexes: Vec<usize>,
}

pub fn select_attachments(
    capability: MediaUnderstandingCapability,
    attachments: &[NormalizedMediaAttachment],
    policy: &AttachmentSelectionPolicy,
    decided_audio_indexes: Option<&[usize]>,
) -> AttachmentSelectionResult {
    let target_kind = capability.as_str();
    let decided_set: std::collections::HashSet<usize> = decided_audio_indexes
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();

    let mut matches: Vec<NormalizedMediaAttachment> = attachments
        .iter()
        .filter(|att| {
            if capability == MediaUnderstandingCapability::Audio
                && (att.already_transcribed || decided_set.contains(&att.index))
            {
                return false;
            }
            resolve_attachment_media_kind(att) == target_kind
        })
        .cloned()
        .collect();

    if matches.is_empty() {
        return AttachmentSelectionResult {
            selected: Vec::new(),
            dropped_attachment_indexes: Vec::new(),
        };
    }

    match policy.prefer {
        Some(AttachmentOrderPreference::Last) => {
            matches.reverse();
        }
        Some(AttachmentOrderPreference::Path) => {
            matches.sort_by_key(|att| if att.path.is_some() { 0 } else { 1 });
        }
        Some(AttachmentOrderPreference::Url) => {
            matches.sort_by_key(|att| if att.url.is_some() { 0 } else { 1 });
        }
        _ => {}
    }

    let limit = match policy.mode {
        AttachmentSelectionMode::All => policy.max_attachments.max(1),
        AttachmentSelectionMode::First => 1,
    };

    let selected: Vec<NormalizedMediaAttachment> = matches.iter().take(limit).cloned().collect();
    let dropped: Vec<usize> = matches.iter().skip(limit).map(|att| att.index).collect();

    AttachmentSelectionResult {
        selected,
        dropped_attachment_indexes: dropped,
    }
}
