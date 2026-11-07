use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::extensions::attachments::media_facts::{is_image_media_fact, MediaFact};

pub const MEDIA_FFMPEG_MAX_BUFFER_BYTES: usize = 10 * 1024 * 1024;
pub const MEDIA_FFPROBE_TIMEOUT_MS: u64 = 10_000;
pub const MEDIA_FFMPEG_TIMEOUT_MS: u64 = 45_000;
pub const MEDIA_FFMPEG_MAX_AUDIO_DURATION_SECS: u64 = 20 * 60;

pub const DEFAULT_OPUS_SAMPLE_RATE_HZ: u32 = 48_000;
pub const DEFAULT_OPUS_BITRATE: &str = "64k";
pub const DEFAULT_OPUS_CHANNELS: u32 = 1;
pub const DEFAULT_TEMP_PREFIX: &str = "audio-opus-";
pub const DEFAULT_OUTPUT_FILE_NAME: &str = "voice.opus";

static VOICE_MESSAGE_EXTENSIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert(".oga");
    s.insert(".ogg");
    s.insert(".opus");
    s.insert(".mp3");
    s.insert(".m4a");
    s
});

static VOICE_MESSAGE_MIMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("audio/ogg");
    s.insert("audio/opus");
    s.insert("audio/mpeg");
    s.insert("audio/mp3");
    s.insert("audio/mp4");
    s.insert("audio/x-m4a");
    s.insert("audio/m4a");
    s
});

static PATH_PARENT_SEGMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\\/])\.\.(?:[\\/]|$)").expect("valid regex"));
static FORWARD_NETWORK_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^//").expect("valid regex"));
static FILE_URL_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^file:(?://)?").expect("valid regex"));

pub const FILE_URL_LOCAL_NETWORK_KEY_PREFIX: &str = "\0file-url-local-network:";

/// Checks whether MIME type or filename is compatible with voice-message delivery.
pub fn is_voice_message_compatible_audio(content_type: Option<&str>, file_name: Option<&str>) -> bool {
    if let Some(ct) = content_type {
        let norm = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        if VOICE_MESSAGE_MIMES.contains(norm.as_str()) {
            return true;
        }
    }

    if let Some(name) = file_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let lower = trimmed.to_ascii_lowercase();
            for ext in VOICE_MESSAGE_EXTENSIONS.iter() {
                if lower.ends_with(ext) {
                    return true;
                }
            }
        }
    }

    false
}

/// Parses codec and positive sample rate from compact ffprobe stream output.
pub fn parse_ffprobe_codec_and_sample_rate(stdout: &str) -> (Option<String>, Option<u32>) {
    let trimmed = stdout.trim();
    let mut parts = trimmed
        .split([',', '\r', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let codec_raw = parts.next().unwrap_or("");
    let sample_raw = parts.next().unwrap_or("");

    let codec = if codec_raw.is_empty() {
        None
    } else {
        Some(codec_raw.to_ascii_lowercase())
    };

    let sample_rate_hz = if !sample_raw.is_empty() && sample_raw.chars().all(|c| c.is_ascii_digit()) {
        sample_raw.parse::<u32>().ok().filter(|&r| r > 0)
    } else {
        None
    };

    (codec, sample_rate_hz)
}

fn normalize_absolute_local_path(val: &str) -> String {
    let p = Path::new(val);
    if !p.is_absolute()
        || PATH_PARENT_SEGMENT_RE.is_match(val)
        || FORWARD_NETWORK_PATH_RE.is_match(val)
    {
        return val.to_string();
    }
    // Clean path separators without resolving symlinks
    let mut components = Vec::new();
    for c in p.components() {
        match c {
            std::path::Component::Normal(s) => components.push(s.to_string_lossy().into_owned()),
            std::path::Component::RootDir => components.push("".to_string()),
            std::path::Component::Prefix(prefix) => components.push(prefix.as_os_str().to_string_lossy().into_owned()),
            _ => return val.to_string(),
        }
    }
    if val.starts_with('/') {
        format!("/{}", components.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/"))
    } else {
        components.join("/")
    }
}

/// Canonicalizes equivalent local media references without resolving the filesystem.
pub fn normalize_media_reference_for_comparison(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if !FILE_URL_PREFIX_RE.is_match(trimmed) {
        return normalize_absolute_local_path(trimmed);
    }

    if let Ok(parsed) = Url::parse(trimmed) {
        if parsed.scheme() == "file" {
            let path_str = parsed.path();
            if path_str.contains("%2f")
                || path_str.contains("%2F")
                || path_str.contains("%5c")
                || path_str.contains("%5C")
            {
                return trimmed.to_string();
            }
            if let Ok(file_path) = parsed.to_file_path() {
                return normalize_absolute_local_path(&file_path.to_string_lossy());
            }
        }
    }

    let remainder = FILE_URL_PREFIX_RE.replace(trimmed, "").to_string();
    let local_path = if remainder.starts_with('/') {
        remainder
    } else if remainder.to_ascii_lowercase().starts_with("localhost/") {
        remainder["localhost".len()..].to_string()
    } else {
        return trimmed.to_string();
    };

    normalize_absolute_local_path(&local_path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptImageOrderEntry {
    Inline,
    Offloaded,
}

/// Stored images require vision; facts already replaced by text descriptions do not.
pub fn has_prompt_image_input(
    images_count: usize,
    image_order: &[PromptImageOrderEntry],
    media: &[MediaFact],
    suppressed_indexes: Option<&[usize]>,
) -> bool {
    if images_count > 0 || !image_order.is_empty() {
        return true;
    }

    let suppressed_set: HashSet<usize> = suppressed_indexes
        .map(|arr| arr.iter().copied().collect())
        .unwrap_or_default();

    media.iter().enumerate().any(|(idx, fact)| {
        is_image_media_fact(fact)
            && fact.hydration_suppressed != Some(true)
            && !suppressed_set.contains(&idx)
    })
}

pub fn normalize_audio_extension(input_extension: Option<&str>, input_file_name: Option<&str>) -> String {
    let candidate = if let Some(ext) = input_extension.map(str::trim).filter(|s| !s.is_empty()) {
        if ext.starts_with('.') {
            ext.to_string()
        } else {
            format!(".{}", ext)
        }
    } else if let Some(name) = input_file_name {
        Path::new(name)
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy()))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let lower = candidate.to_ascii_lowercase();
    if lower.len() >= 2 && lower.len() <= 13 && lower.starts_with('.') && lower[1..].chars().all(|c| c.is_ascii_alphanumeric()) {
        lower
    } else {
        ".audio".to_string()
    }
}

pub fn normalize_temp_prefix(val: Option<&str>) -> String {
    let Some(raw) = val.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_TEMP_PREFIX.to_string();
    };
    let sanitized: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        return DEFAULT_TEMP_PREFIX.to_string();
    }
    if sanitized.ends_with('-') {
        sanitized
    } else {
        format!("{}-", sanitized)
    }
}

pub fn normalize_output_file_name(val: Option<&str>) -> String {
    let Some(raw) = val.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_OUTPUT_FILE_NAME.to_string();
    };
    let base = Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| DEFAULT_OUTPUT_FILE_NAME.to_string());
    if base.len() >= 1 && base.len() <= 80 && base != "." && base != ".." && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
        base
    } else {
        DEFAULT_OUTPUT_FILE_NAME.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusTranscodeArgs {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub bitrate: String,
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub max_duration_seconds: Option<u64>,
}

pub fn build_ffmpeg_opus_args(args: &OpusTranscodeArgs) -> Vec<String> {
    let mut cmd = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        args.input_path.to_string_lossy().into_owned(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
    ];
    if let Some(dur) = args.max_duration_seconds {
        cmd.push("-t".into());
        cmd.push(dur.to_string());
    }
    cmd.extend(vec![
        "-c:a".into(),
        "libopus".into(),
        "-b:a".into(),
        args.bitrate.clone(),
        "-ar".into(),
        args.sample_rate_hz.to_string(),
        "-ac".into(),
        args.channels.to_string(),
        "-f".into(),
        "opus".into(),
        args.output_path.to_string_lossy().into_owned(),
    ]);
    cmd
}
