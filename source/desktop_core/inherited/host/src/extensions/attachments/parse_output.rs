//! Text output parsing for MEDIA directives, inline images, and voice tags.
//!
//! Maps upstream OpenClaw modules:
//! - `src/media/parse.ts`
//! - `src/media/parse-output.ts`
//! - `src/media/parse.test.ts`

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParsedMediaOutputSegment {
    Text { text: String },
    Media { url: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitMediaOutputOptions {
    #[serde(default = "default_true")]
    pub extract_audio_directives: bool,
    #[serde(default = "default_true")]
    pub extract_media_directives: bool,
    #[serde(default)]
    pub preserve_trailing_whitespace: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitMediaFromOutputOptions {
    #[serde(default = "default_true")]
    pub extract_audio_directives: bool,
    #[serde(default)]
    pub extract_markdown_images: bool,
    #[serde(default = "default_true")]
    pub extract_media_directives: bool,
    #[serde(default)]
    pub preserve_trailing_whitespace: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown_image_allowlist: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMediaOutput {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub media_urls: Vec<String>,
    #[serde(skip_serializing_if = "is_zero", default)]
    pub rejected_media_count: usize,
    #[serde(skip_serializing_if = "is_false", default)]
    pub audio_as_voice: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub segments: Vec<ParsedMediaOutputSegment>,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

pub fn is_relative_assistant_media_reference(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:image/")
        || lower.starts_with("data:audio/")
        || lower.starts_with("data:video/")
        || lower.starts_with("file:")
        || lower.starts_with('~')
        || lower.starts_with('/')
        || (lower.len() >= 3
            && lower.chars().next().unwrap().is_ascii_alphabetic()
            && lower.chars().nth(1) == Some(':')
            && (lower.chars().nth(2) == Some('/') || lower.chars().nth(2) == Some('\\'))))
}

pub fn normalize_media_source(src: &str) -> String {
    let trimmed = src.trim();
    if let Some(rest) = trimmed.strip_prefix("file://") {
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("file:") {
        rest.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn has_traversal_or_unsupported_home_dir_prefix(candidate: &str) -> bool {
    if candidate.starts_with('~') && !candidate.starts_with("~/") && !candidate.starts_with("~\\") {
        return true;
    }
    for segment in candidate.split(['/', '\\']) {
        if segment == ".." {
            return true;
        }
    }
    false
}

pub fn looks_like_local_file_path(candidate: &str) -> bool {
    if candidate.starts_with('/')
        || candidate.starts_with("./")
        || candidate.starts_with(".\\")
        || candidate.starts_with("../")
        || candidate.starts_with("..\\")
        || candidate.starts_with('~')
        || candidate.starts_with("\\\\")
    {
        return true;
    }
    let chars: Vec<char> = candidate.chars().collect();
    if chars.len() >= 3 && chars[0].is_ascii_alphabetic() && chars[1] == ':' && (chars[2] == '/' || chars[2] == '\\') {
        return true;
    }
    if !candidate.contains("://") && (candidate.contains('/') || candidate.contains('\\')) {
        return true;
    }
    false
}

pub fn is_blocked_remote_media_hostname(hostname: &str) -> bool {
    let norm = hostname.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
    if norm.is_empty() {
        return true;
    }
    if norm == "localhost"
        || norm == "localhost.localdomain"
        || norm.ends_with(".localhost")
        || norm.ends_with(".local")
        || norm.ends_with(".internal")
    {
        return true;
    }
    if let Ok(ip) = norm.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(ipv4) => {
                return ipv4.is_loopback() || ipv4.is_private() || ipv4.is_link_local() || ipv4.is_broadcast() || ipv4.is_documentation();
            }
            IpAddr::V6(ipv6) => {
                return ipv6.is_loopback() || ipv6.is_multicast();
            }
        }
    }
    if !norm.contains('.') {
        return true;
    }
    false
}

pub fn is_allowed_remote_media_url(candidate: &str) -> bool {
    let Ok(parsed) = Url::parse(candidate) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    let Some(host_str) = parsed.host_str() else {
        return false;
    };
    !is_blocked_remote_media_hostname(host_str)
}

fn has_file_extension(candidate: &str) -> bool {
    Path::new(candidate).extension().is_some()
}

pub fn is_valid_media(source: &str, allow_spaces: bool, allow_bare_filename: bool) -> bool {
    let candidate = normalize_media_source(source);
    if candidate.is_empty() || candidate.len() > 4096 {
        return false;
    }
    if !allow_spaces && candidate.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let lower = candidate.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return is_allowed_remote_media_url(&candidate);
    }
    if lower.starts_with("media://") {
        return true;
    }
    if has_traversal_or_unsupported_home_dir_prefix(&candidate) {
        return false;
    }
    if looks_like_local_file_path(&candidate) {
        return true;
    }
    if allow_bare_filename && !candidate.contains("://") && has_file_extension(&candidate) {
        return true;
    }
    false
}

fn clean_candidate(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed.trim_start_matches(|c| matches!(c, '`' | '"' | '\'' | '[' | '{' | '('))
        .trim_end_matches(|c| matches!(c, '`' | '"' | '\'' | '\\' | '}' | ')' | ']' | ','));
    stripped.to_string()
}

/// Splits stdout or agent message text into visible prose, media attachments, and optional voice flag.
pub fn split_media_from_output(
    raw: &str,
    options: &SplitMediaFromOutputOptions,
) -> ParsedMediaOutput {
    let internal_opts = SplitMediaOutputOptions {
        extract_audio_directives: options.extract_audio_directives,
        extract_media_directives: options.extract_media_directives,
        preserve_trailing_whitespace: options.preserve_trailing_whitespace,
    };
    split_media_output_internal(raw, &internal_opts, options.markdown_image_allowlist.as_deref())
}

pub fn split_media_output(
    raw: &str,
    options: &SplitMediaOutputOptions,
) -> ParsedMediaOutput {
    split_media_output_internal(raw, options, None)
}

fn split_media_output_internal(
    raw: &str,
    options: &SplitMediaOutputOptions,
    markdown_allowlist: Option<&[String]>,
) -> ParsedMediaOutput {
    let trimmed_raw = if options.preserve_trailing_whitespace {
        raw.to_string()
    } else {
        raw.trim_end().to_string()
    };

    if trimmed_raw.trim().is_empty() {
        return ParsedMediaOutput {
            text: if options.preserve_trailing_whitespace { trimmed_raw } else { String::new() },
            media_urls: Vec::new(),
            rejected_media_count: 0,
            audio_as_voice: false,
            segments: Vec::new(),
        };
    }

    let may_contain_media = options.extract_media_directives && trimmed_raw.to_ascii_lowercase().contains("media:");
    let may_contain_audio = options.extract_audio_directives && trimmed_raw.contains("[[audio_as_voice]]");
    let may_contain_md_img = markdown_allowlist.is_some() && trimmed_raw.contains("![");

    if !may_contain_media && !may_contain_audio && !may_contain_md_img {
        return ParsedMediaOutput {
            text: trimmed_raw.clone(),
            media_urls: Vec::new(),
            rejected_media_count: 0,
            audio_as_voice: false,
            segments: vec![ParsedMediaOutputSegment::Text { text: trimmed_raw }],
        };
    }

    let mut media_urls = Vec::new();
    let mut rejected_media_count = 0;
    let mut segments = Vec::new();
    let mut kept_lines = Vec::new();
    let mut in_code_fence = false;

    for line in trimmed_raw.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.starts_with("```") {
            in_code_fence = !in_code_fence;
            kept_lines.push(line.to_string());
            segments.push(ParsedMediaOutputSegment::Text { text: line.to_string() });
            continue;
        }

        if in_code_fence {
            kept_lines.push(line.to_string());
            segments.push(ParsedMediaOutputSegment::Text { text: line.to_string() });
            continue;
        }

        let is_media_directive = options.extract_media_directives && trimmed_line.to_ascii_lowercase().starts_with("media:");
        if is_media_directive {
            let payload = trimmed_line["media:".len()..].trim().trim_matches('`');
            let parts: Vec<&str> = if payload.starts_with('[') && payload.ends_with(']') {
                payload[1..payload.len() - 1].split(',').map(str::trim).collect()
            } else if payload.contains(' ') {
                payload.split_whitespace().collect()
            } else {
                vec![payload]
            };

            let mut valid_any = false;
            for part in parts {
                let cand = clean_candidate(part);
                if is_valid_media(&cand, true, true) {
                    media_urls.push(cand.clone());
                    segments.push(ParsedMediaOutputSegment::Media { url: cand });
                    valid_any = true;
                } else if looks_like_local_file_path(&cand) || cand.starts_with("http") {
                    rejected_media_count += 1;
                }
            }

            if !valid_any && rejected_media_count == 0 {
                kept_lines.push(line.to_string());
                segments.push(ParsedMediaOutputSegment::Text { text: line.to_string() });
            }
            continue;
        }

        // Markdown image handling if allowlist present
        if let Some(allowlist) = markdown_allowlist {
            if line.contains("![") && line.contains("](") {
                let mut last_idx = 0;
                let mut line_buf = String::new();
                let chars: Vec<char> = line.chars().collect();
                let mut i = 0;
                let mut handled_img = false;
                while i + 1 < chars.len() {
                    if chars[i] == '!' && chars[i + 1] == '[' {
                        if let Some(close_bracket) = line[i..].find("](") {
                            let link_start = i + close_bracket + 2;
                            if let Some(close_paren) = line[link_start..].find(')') {
                                let link = &line[link_start..link_start + close_paren];
                                let cand = clean_candidate(link);
                                if allowlist.iter().any(|a| a.trim() == cand) || (cand.starts_with("https://") && is_allowed_remote_media_url(&cand)) {
                                    let before = &line[last_idx..i];
                                    let before_text = if options.preserve_trailing_whitespace {
                                        before.to_string()
                                    } else {
                                        clean_candidate(before)
                                    };
                                    if !before_text.trim().is_empty() {
                                        segments.push(ParsedMediaOutputSegment::Text { text: before_text });
                                    }
                                    media_urls.push(cand.clone());
                                    segments.push(ParsedMediaOutputSegment::Media { url: cand });
                                    line_buf.push_str(before);
                                    last_idx = link_start + close_paren + 1;
                                    i = last_idx;
                                    handled_img = true;
                                    continue;
                                }
                            }
                        }
                    }
                    i += 1;
                }
                if handled_img {
                    let after = &line[last_idx..];
                    let after_text = if options.preserve_trailing_whitespace {
                        after.to_string()
                    } else {
                        let mut c = after.trim().to_string();
                        while c.contains("  ") {
                            c = c.replace("  ", " ");
                        }
                        c
                    };
                    if !after_text.trim().is_empty() {
                        segments.push(ParsedMediaOutputSegment::Text { text: after_text });
                    }
                    line_buf.push_str(after);
                    let mut clean = line_buf.trim().to_string();
                    if !options.preserve_trailing_whitespace {
                        while clean.contains("  ") {
                            clean = clean.replace("  ", " ");
                        }
                    }
                    if !clean.is_empty() {
                        kept_lines.push(clean);
                    }
                    continue;
                }
            }
        }

        kept_lines.push(line.to_string());
        segments.push(ParsedMediaOutputSegment::Text { text: line.to_string() });
    }

    let mut final_text = kept_lines.join("\n");
    let mut audio_as_voice = false;
    if options.extract_audio_directives && final_text.contains("[[audio_as_voice]]") {
        audio_as_voice = true;
        final_text = final_text.replace("[[audio_as_voice]]", "");
        if !options.preserve_trailing_whitespace {
            final_text = final_text.trim_end().to_string();
        }
    }

    ParsedMediaOutput {
        text: final_text,
        media_urls,
        rejected_media_count,
        audio_as_voice,
        segments,
    }
}
