use std::collections::HashSet;

pub const DEFAULT_LINK_TIMEOUT_SECONDS: u64 = 30;
pub const DEFAULT_MAX_LINKS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkUnderstandingModelConfig {
    pub command: String,
    pub args: Vec<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkUnderstandingConfig {
    pub enabled: bool,
    pub max_links: Option<usize>,
    pub timeout_seconds: Option<u64>,
    pub models: Vec<LinkUnderstandingModelConfig>,
}

impl Default for LinkUnderstandingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_links: Some(DEFAULT_MAX_LINKS),
            timeout_seconds: Some(DEFAULT_LINK_TIMEOUT_SECONDS),
            models: Vec::new(),
        }
    }
}

pub fn is_link_url_template(value: &str) -> bool {
    value.contains("LinkUrl") || value.contains("LinkFinalUrl")
}

pub fn is_blocked_link_host(host: &str) -> bool {
    let lower = host.trim().to_lowercase();
    lower == "localhost"
        || lower == "127.0.0.1"
        || lower == "::1"
        || lower == "0.0.0.0"
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
}

/// Finds spans `[start, end]` of markdown inline links `[...](...)` to exclude citations.
pub fn find_markdown_link_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let start = i;
            let mut j = i + 1;
            let mut depth = 1;
            while j < bytes.len() && depth > 0 {
                if bytes[j] == b'[' {
                    depth += 1;
                } else if bytes[j] == b']' {
                    depth -= 1;
                }
                j += 1;
            }
            if depth == 0 && j < bytes.len() && bytes[j] == b'(' {
                let mut k = j + 1;
                while k < bytes.len() && bytes[k] != b')' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b')' {
                    spans.push((start, k + 1));
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

pub fn extract_bare_links_from_message(message: &str, max_links: usize) -> Vec<String> {
    let limit = if max_links == 0 { DEFAULT_MAX_LINKS } else { max_links };
    let markdown_spans = find_markdown_link_spans(message);

    // Mask out markdown link spans with spaces
    let mut masked = message.to_string();
    for (start, end) in markdown_spans {
        if start < masked.len() && end <= masked.len() {
            let spaces = " ".repeat(end - start);
            masked.replace_range(start..end, &spaces);
        }
    }

    let mut links = Vec::new();
    let mut seen = HashSet::new();

    for token in masked.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| c.is_ascii_punctuation() && c != '/' && c != ':');
        if let Ok(parsed) = url::Url::parse(trimmed) {
            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                continue;
            }
            if let Some(host) = parsed.host_str() {
                if is_blocked_link_host(host) {
                    continue;
                }
            } else {
                continue;
            }
            let url_str = parsed.as_str().to_string();
            if seen.insert(url_str.clone()) {
                links.push(url_str);
                if links.len() >= limit {
                    break;
                }
            }
        }
    }

    links
}

pub fn enrich_message_body_with_links(body: Option<&str>, link_outputs: &[String]) -> String {
    let suffix = link_outputs.join("\n");
    let base = body.unwrap_or("").trim();
    if base.is_empty() {
        suffix
    } else if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{}\n\n{}", base, suffix)
    }
}
