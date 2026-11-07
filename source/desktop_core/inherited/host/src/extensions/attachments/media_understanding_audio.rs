use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use super::MediaUnderstandingCapability;

pub const EMPTY_VOICE_NOTE_PLACEHOLDER: &str =
    "[Voice note could not be transcribed because the audio attachment was too small]";

pub const DEFAULT_ECHO_TRANSCRIPT_FORMAT: &str = "📝 \"{transcript}\"";
pub const OPENAI_AUDIO_TRANSCRIPTIONS_API: &str = "openai-audio-transcriptions";
pub const MIN_AUDIO_FILE_BYTES: u64 = 100;

/// Empty STT prompt echoes are not speech; match whole utterances, never prose fragments.
pub fn is_transcript_artifact_text(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    let normalized = words.join(" ").to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "" | "context:" | "###" | "transcribe the audio" | "transcribe the audio."
    )
}

/// Resolves the multipart upload filename, mapping AAC inputs to provider-friendly `.m4a`.
pub fn resolve_audio_transcription_upload_file_name(
    file_name: Option<&str>,
    mime: Option<&str>,
) -> String {
    let trimmed = file_name.map(str::trim).filter(|s| !s.is_empty());
    let base_name = match trimmed {
        Some(t) => Path::new(t)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("audio"),
        None => "audio",
    };

    let lower_mime = mime.map(|m| m.trim().to_ascii_lowercase());

    if base_name.to_ascii_lowercase().ends_with(".aac") {
        let stem = &base_name[..base_name.len() - 4];
        let prefix = if stem.is_empty() { "audio" } else { stem };
        return format!("{prefix}.m4a");
    }

    if Path::new(base_name).extension().is_none() && lower_mime.as_deref() == Some("audio/aac") {
        let prefix = if base_name.is_empty() { "audio" } else { base_name };
        return format!("{prefix}.m4a");
    }

    base_name.to_string()
}

pub fn build_open_ai_compatible_auth_headers(
    api_key: Option<&str>,
) -> Option<HashMap<String, String>> {
    let key = api_key?.trim();
    if key.is_empty() {
        return None;
    }
    let mut map = HashMap::new();
    map.insert("authorization".to_string(), format!("Bearer {key}"));
    Some(map)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInputBudget {
    pub max_bytes: usize,
    pub max_duration_seconds: u32,
}

impl Default for AudioInputBudget {
    fn default() -> Self {
        Self {
            max_bytes: 25 * 1024 * 1024, // 25 MiB standard limit
            max_duration_seconds: 600,    // 10 minutes
        }
    }
}

impl AudioInputBudget {
    pub fn validate_payload_size(&self, byte_count: usize) -> Result<(), String> {
        if byte_count > self.max_bytes {
            Err(format!(
                "Audio payload size {byte_count} exceeds maximum allowed budget {}",
                self.max_bytes
            ))
        } else {
            Ok(())
        }
    }
}

pub fn resolve_audio_input_budget_for_config(
    audio_enabled: bool,
    max_bytes: Option<usize>,
) -> Option<usize> {
    if !audio_enabled {
        None
    } else {
        Some(max_bytes.unwrap_or(25 * 1024 * 1024))
    }
}

/// Formats the transcript into the echo template, preserving literal dollar sequences.
pub fn format_echo_transcript(transcript: &str, format: Option<&str>) -> String {
    let template = format.unwrap_or(DEFAULT_ECHO_TRANSCRIPT_FORMAT);
    template.replace("{transcript}", transcript)
}

/// Checks if a message channel is a valid deliverable destination.
pub fn is_deliverable_message_channel(channel: &str) -> bool {
    let normalized = channel.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "discord" | "telegram" | "whatsapp" | "slack" | "signal" | "imessage" | "wechat"
        | "mattermost" | "matrix" | "teams" => true,
        _ => false,
    }
}

/// Evaluates if echo transcript delivery should proceed based on destination context.
pub fn can_deliver_transcript_echo(channel: Option<&str>, to: Option<&str>) -> bool {
    let chan = match channel {
        Some(c) if !c.trim().is_empty() => c.trim(),
        _ => return false,
    };
    let dest = match to {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return false,
    };
    !dest.is_empty() && is_deliverable_message_channel(chan)
}

pub fn resolve_openai_audio_auth_model_api(
    capability: MediaUnderstandingCapability,
    provider_id: &str,
) -> Option<&'static str> {
    if capability == MediaUnderstandingCapability::Audio
        && provider_id.trim().eq_ignore_ascii_case("openai")
    {
        Some(OPENAI_AUDIO_TRANSCRIPTIONS_API)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTranscriptionFormData {
    pub file_name: String,
    pub mime: String,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
}

pub fn build_audio_transcription_form_data(
    file_name: &str,
    mime: &str,
    model: &str,
    language: Option<&str>,
    prompt: Option<&str>,
) -> AudioTranscriptionFormData {
    let remapped_name = resolve_audio_transcription_upload_file_name(Some(file_name), Some(mime));
    AudioTranscriptionFormData {
        file_name: remapped_name,
        mime: mime.to_string(),
        model: model.to_string(),
        language: language.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        prompt: prompt.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
    }
}

pub fn command_id(command: &str) -> String {
    let trimmed = command.trim();
    let basename = trimmed
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(trimmed);
    let stem = if let Some(idx) = basename.rfind('.') {
        let ext = &basename[idx..];
        if ext.eq_ignore_ascii_case(".exe")
            || ext.eq_ignore_ascii_case(".com")
            || ext.eq_ignore_ascii_case(".cmd")
            || ext.eq_ignore_ascii_case(".bat")
        {
            &basename[..idx]
        } else {
            basename
        }
    } else {
        basename
    };
    stem.to_ascii_lowercase()
}

pub fn resolve_requested_local_audio_backend(command: &str, args: &[String]) -> Option<String> {
    let cmd = command_id(command);
    if cmd == "sherpa-onnx-offline" {
        for (i, arg) in args.iter().enumerate() {
            if arg == "--provider" {
                if let Some(next) = args.get(i + 1) {
                    return Some(next.trim().to_ascii_lowercase());
                }
            } else if let Some(stripped) = arg.strip_prefix("--provider=") {
                return Some(stripped.trim().to_ascii_lowercase());
            }
        }
        return Some("cpu".to_string());
    }
    if cmd == "whisper-cli" {
        if args.iter().any(|arg| arg == "-ng" || arg == "--no-gpu") {
            return Some("cpu".to_string());
        }
        for (i, arg) in args.iter().enumerate() {
            if arg == "-dev" || arg == "--device" {
                if let Some(next) = args.get(i + 1) {
                    let dev = next.trim();
                    if !dev.is_empty() {
                        return Some(format!("device:{dev}"));
                    }
                }
            } else if let Some(stripped) = arg.strip_prefix("--device=") {
                let dev = stripped.trim();
                if !dev.is_empty() {
                    return Some(format!("device:{dev}"));
                }
            }
        }
        return None;
    }
    None
}

static ACCELERATOR_FAILED_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)failed to initialize\s+(?:MTL\d+|Metal|CUDA\d*)\s+backend")
        .expect("valid regex")
});

static USING_METAL_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)using\s+(?:MTL\d+|Metal)\s+backend").expect("valid regex")
});

static USING_CUDA_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)using\s+CUDA\d*\s+backend").expect("valid regex")
});

static USING_CPU_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)using\s+CPU\s+backend|no GPU found").expect("valid regex")
});

pub fn record_local_audio_backend_observation(command: &str, output: &str) -> Option<&'static str> {
    if command_id(command) != "whisper-cli" {
        return None;
    }
    if ACCELERATOR_FAILED_RE.is_match(output) {
        return Some("cpu");
    }
    if USING_METAL_RE.is_match(output) {
        return Some("metal");
    }
    if USING_CUDA_RE.is_match(output) {
        return Some("cuda");
    }
    if USING_CPU_RE.is_match(output) {
        return Some("cpu");
    }
    None
}

pub fn rank_local_audio_candidate(candidate_id: &str, observed_backend: Option<&str>) -> usize {
    if candidate_id == "whisper-cli" && (observed_backend == Some("metal") || observed_backend == Some("cuda")) {
        0
    } else if candidate_id == "sherpa-onnx-offline" {
        1
    } else if candidate_id == "whisper-cli" {
        2
    } else if candidate_id == "parakeet-mlx" {
        3
    } else {
        4
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioCandidate {
    pub id: String,
    pub command: String,
    pub available: bool,
    pub ready: bool,
    pub capable_backend: Option<String>,
    pub requested_backend: Option<String>,
    pub observed_backend: Option<String>,
    pub evidence: String,
    pub selected: bool,
    pub reason: Option<String>,
    pub entry_command: Option<String>,
    pub entry_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAudioSelection {
    pub candidates: Vec<LocalAudioCandidate>,
    pub selected: Option<LocalAudioCandidate>,
}

pub fn format_local_audio_selection(selection: &LocalAudioSelection) -> Option<String> {
    let selected = selection.selected.as_ref()?;
    let describe_backend = |c: &LocalAudioCandidate| {
        let mut parts = Vec::new();
        if let Some(ref cap) = c.capable_backend {
            parts.push(format!("capable={cap}"));
        }
        if let Some(ref req) = c.requested_backend {
            parts.push(format!("requested={req}"));
        }
        parts.push(format!(
            "observed={}",
            c.observed_backend.as_deref().unwrap_or("unknown")
        ));
        parts.join(", ")
    };

    let fallbacks: Vec<String> = selection
        .candidates
        .iter()
        .filter(|c| c.ready && c.id != selected.id)
        .map(|c| format!("{} ({})", c.command, describe_backend(c)))
        .collect();

    let fallbacks_str = if fallbacks.is_empty() {
        String::new()
    } else {
        format!("; fallbacks: {}", fallbacks.join(", "))
    };

    Some(format!(
        "{} ({}); {}{fallbacks_str}",
        selected.command,
        describe_backend(selected),
        selected.evidence
    ))
}

pub fn extract_sherpa_onnx_text(raw: &str) -> Option<String> {
    fn try_parse(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if !trimmed.starts_with('{') && !trimmed.starts_with('"') {
            return None;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(s) = val.as_str() {
                return try_parse(s);
            }
            if let Some(text) = val.get("text").and_then(|t| t.as_str()) {
                let trimmed_text = text.trim();
                return Some(trimmed_text.to_string());
            }
        }
        None
    }

    if let Some(direct) = try_parse(raw) {
        return Some(direct);
    }
    for line in raw.lines().rev() {
        if let Some(parsed) = try_parse(line) {
            return Some(parsed);
        }
    }
    None
}

pub fn resolve_whisper_output_path(args: &[String], media_path: &str) -> Option<PathBuf> {
    let mut output_dir: Option<&str> = None;
    let mut output_format: Option<&str> = None;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--output_dir" || arg == "-o" {
            if idx + 1 < args.len() {
                output_dir = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("--output_dir=") {
            output_dir = Some(stripped);
        } else if let Some(stripped) = arg.strip_prefix("-o=") {
            output_dir = Some(stripped);
        } else if arg == "--output_format" || arg == "-f" {
            if idx + 1 < args.len() {
                output_format = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("--output_format=") {
            output_format = Some(stripped);
        } else if let Some(stripped) = arg.strip_prefix("-f=") {
            output_format = Some(stripped);
        }
        idx += 1;
    }
    let dir = output_dir?;
    let fmt = output_format.unwrap_or("all");
    if fmt != "txt" && fmt != "all" {
        return None;
    }
    let media_stem = Path::new(media_path).file_stem()?.to_str()?;
    Some(Path::new(dir).join(format!("{}.txt", media_stem)))
}

pub fn resolve_whisper_cpp_output_path(args: &[String]) -> Option<PathBuf> {
    if !args.iter().any(|arg| arg == "-otxt" || arg == "--output-txt") {
        return None;
    }
    let mut output_base: Option<&str> = None;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "-of" || arg == "--output-file" {
            if idx + 1 < args.len() {
                output_base = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("-of=") {
            output_base = Some(stripped);
        } else if let Some(stripped) = arg.strip_prefix("--output-file=") {
            output_base = Some(stripped);
        }
        idx += 1;
    }
    let base = output_base?;
    Some(PathBuf::from(format!("{}.txt", base)))
}

pub fn resolve_parakeet_output_path(args: &[String], media_path: &str) -> Option<PathBuf> {
    let mut output_dir: Option<&str> = None;
    let mut output_format: Option<&str> = None;
    let mut output_template: Option<&str> = None;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--output-dir" {
            if idx + 1 < args.len() {
                output_dir = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("--output-dir=") {
            output_dir = Some(stripped);
        } else if arg == "--output-format" {
            if idx + 1 < args.len() {
                output_format = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("--output-format=") {
            output_format = Some(stripped);
        } else if arg == "--output-template" {
            if idx + 1 < args.len() {
                output_template = Some(&args[idx + 1]);
                idx += 2;
                continue;
            }
        } else if let Some(stripped) = arg.strip_prefix("--output-template=") {
            output_template = Some(stripped);
        }
        idx += 1;
    }
    let dir = output_dir?;
    let fmt = output_format.unwrap_or("srt");
    let tmpl = output_template.unwrap_or("{filename}");
    if (fmt != "txt" && fmt != "all") || tmpl != "{filename}" {
        return None;
    }
    let media_stem = Path::new(media_path).file_stem()?.to_str()?;
    Some(Path::new(dir).join(format!("{}.txt", media_stem)))
}
