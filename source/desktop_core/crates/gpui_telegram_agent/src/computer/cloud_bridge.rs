use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

/// Boxed `Send` future alias used by [`ComputerCloudProvider`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Summary of a controllable local device or cloud computer exposed to Hermes tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentComputerNodeSummary {
    pub node_id: String,
    pub kind: String,
    pub display_name: String,
    pub ip_address: String,
    pub host: Option<String>,
    pub os_name: String,
    pub online: bool,
    pub is_current_device: bool,
    pub bound_to_this_bot: bool,
    pub run_policy: String,
    pub tier_badge: String,
}

/// Configuration payload for connecting and registering a cloud computer/sandbox node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentCloudServerConfigInput {
    pub node_id: Option<String>,
    pub display_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub secret: String,
    pub agent_endpoint_url: Option<String>,
    pub workdir: Option<String>,
    pub bind_to_bot: bool,
}

impl Default for AgentCloudServerConfigInput {
    fn default() -> Self {
        Self {
            node_id: None,
            display_name: String::new(),
            host: String::new(),
            port: 22,
            username: "root".to_string(),
            auth_method: "ssh_password".to_string(),
            secret: String::new(),
            agent_endpoint_url: None,
            workdir: None,
            bind_to_bot: true,
        }
    }
}

/// Remote operation request executed via [`ComputerCloudProvider`].
///
/// Canonical `op` values emitted by Hermes computer tools:
/// - `"shell_exec"` (`command`, optional `cwd`)
/// - `"read_file"` (`path`)
/// - `"write_file"` (`path`, `content`)
/// - `"list_dir"` (`path`)
/// - `"capture_screen"`
/// - `"click"` / `"move"` (`x`, `y`)
/// - `"type"` (`text`)
/// - `"key"` (`key`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentComputerExecInput {
    pub node_id: Option<String>,
    pub caller_bot_id: Option<u64>,
    pub op: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub path: Option<String>,
    pub content: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub text: Option<String>,
    pub key: Option<String>,
}

/// Output produced by [`ComputerCloudProvider::execute_on_computer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentComputerExecOutput {
    pub node_id: String,
    pub ok: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub entries: Vec<String>,
    pub frame_data_url: Option<String>,
    pub executed_by: String,
}

/// Bridge trait implemented by the server/runtime control plane to expose logged-in
/// local devices and configured cloud servers to Hermes computer tools.
pub trait ComputerCloudProvider: Send + Sync {
    fn list_computers<'a>(
        &'a self,
        user_id: u64,
        bot_id: Option<u64>,
    ) -> BoxFuture<'a, gpui_telegram_core::Result<Vec<AgentComputerNodeSummary>>>;

    fn bind_bot_computer<'a>(
        &'a self,
        user_id: u64,
        bot_id: u64,
        node_id: Option<String>,
    ) -> BoxFuture<'a, gpui_telegram_core::Result<AgentComputerNodeSummary>>;

    fn configure_cloud_server<'a>(
        &'a self,
        user_id: u64,
        bot_id: Option<u64>,
        cfg: AgentCloudServerConfigInput,
    ) -> BoxFuture<'a, gpui_telegram_core::Result<AgentComputerNodeSummary>>;

    fn execute_on_computer<'a>(
        &'a self,
        user_id: u64,
        req: AgentComputerExecInput,
    ) -> BoxFuture<'a, gpui_telegram_core::Result<AgentComputerExecOutput>>;
}

/// Parses natural-language cloud server credentials from a user chat prompt.
///
/// Returns `Some(...)` ONLY when both a valid host/URL AND a non-empty secret
/// (`password`, `token`, or PEM private key) are present so ordinary chat turns
/// never false-trigger cloud configuration.
pub fn parse_cloud_credentials_from_prompt(prompt: &str) -> Option<AgentCloudServerConfigInput> {
    let (pem_key, scrubbed_prompt) = extract_pem_private_key(prompt);
    let urls = extract_http_urls(&scrubbed_prompt);

    let agent_url_explicit = extract_keyed_value_matching(
        &scrubbed_prompt,
        &["agent_endpoint_url", "agent_url", "agent", "endpoint"],
        |v| v.starts_with("http://") || v.starts_with("https://"),
    )
    .map(|v| trim_trailing_punct(&v).to_string());

    let agent_url = agent_url_explicit.or_else(|| urls.first().cloned());

    let mut host = extract_keyed_value_matching(
        &scrubbed_prompt,
        &["host", "ip", "server", "服务器", "主机", "地址"],
        |v| is_valid_host_or_url_candidate(trim_trailing_punct(v)),
    )
    .map(|v| trim_trailing_punct(&v).to_string());

    let mut parsed_port = extract_keyed_value_matching(
        &scrubbed_prompt,
        &["port", "端口"],
        |v| {
            let digits = v.trim_matches(|c: char| !c.is_ascii_digit());
            !digits.is_empty() && digits.parse::<u16>().is_ok()
        },
    )
    .and_then(|v| v.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u16>().ok());

    let mut username = extract_keyed_value(
        &scrubbed_prompt,
        &["username", "user", "用户名", "用户"],
    )
    .map(|v| trim_trailing_punct(&v).to_string());

    let password = extract_keyed_value(
        &scrubbed_prompt,
        &["password", "passwd", "pwd", "pass", "密码"],
    );

    let token = extract_keyed_value(
        &scrubbed_prompt,
        &["agent_token", "access_token", "token"],
    );

    let display_name = extract_keyed_value(
        &scrubbed_prompt,
        &["display_name", "name", "名称", "别名"],
    )
    .map(|v| trim_trailing_punct(&v).to_string());

    let workdir = extract_keyed_value(
        &scrubbed_prompt,
        &["workdir", "工作目录", "目录", "cwd"],
    )
    .map(|v| trim_trailing_punct(&v).to_string());

    // Handle user@host[:port] syntax if present.
    if let Some((u, h, p)) = extract_user_at_host(&scrubbed_prompt) {
        if username.is_none() {
            username = Some(u);
        }
        if host.is_none() {
            host = Some(h);
        }
        if parsed_port.is_none() {
            parsed_port = p;
        }
    }

    // Handle inline host:port from explicit host field (e.g. host: 192.168.1.10:2222).
    if let Some(raw_h) = host.clone() {
        if raw_h.starts_with("http://") || raw_h.starts_with("https://") {
            if let Some((uh, up)) = parse_url_host_port(&raw_h) {
                host = Some(uh);
                if parsed_port.is_none() {
                    parsed_port = up;
                }
            }
        } else if let Some((h, p)) = split_host_and_port(&raw_h) {
            host = Some(h);
            if parsed_port.is_none() {
                parsed_port = Some(p);
            }
        }
    }

    // Extract host/port from HTTP(S) URL if host is still missing.
    let mut url_port = None;
    if let Some(ref url) = agent_url {
        if let Some((uh, up)) = parse_url_host_port(url) {
            if host.is_none() {
                host = Some(uh);
            }
            url_port = up;
        }
    }

    // Bare IPv4[:port] fallback anywhere in the prompt.
    if host.is_none() {
        if let Some((ip, p)) = extract_first_ipv4(&scrubbed_prompt) {
            host = Some(ip);
            if parsed_port.is_none() {
                parsed_port = p;
            }
        }
    }

    let host = host
        .map(|h| h.trim().to_string())
        .filter(|h| is_valid_host_token(h))?;

    // Determine authentication method and secret. Requires a non-empty secret.
    let has_agent_intent = agent_url.is_some()
        || prompt_contains_ci(&scrubbed_prompt, "agent_url")
        || prompt_contains_ci(&scrubbed_prompt, "agent_http")
        || prompt_contains_ci(&scrubbed_prompt, "agent");

    let (auth_method, secret, default_port, final_agent_url) = if let Some(pem) = pem_key {
        (
            "ssh_private_key".to_string(),
            pem,
            22u16,
            agent_url,
        )
    } else if let Some(tok) = token.filter(|t| !is_empty_or_placeholder_secret(t)) {
        if agent_url.is_some() || has_agent_intent {
            let port_for_url = parsed_port.or(url_port).unwrap_or(8080);
            let resolved_url = agent_url.unwrap_or_else(|| format!("http://{host}:{port_for_url}"));
            (
                "agent_http_token".to_string(),
                tok,
                url_port.unwrap_or(8080),
                Some(resolved_url),
            )
        } else {
            return None;
        }
    } else if let Some(pwd) = password.filter(|p| !is_empty_or_placeholder_secret(p)) {
        ("ssh_password".to_string(), pwd, 22u16, agent_url)
    } else {
        return None;
    };

    let port = parsed_port
        .or_else(|| {
            if auth_method == "agent_http_token" {
                url_port
            } else {
                None
            }
        })
        .unwrap_or(default_port);

    let username = username
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "root".to_string());

    let display_name = display_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Cloud-{host}"));

    Some(AgentCloudServerConfigInput {
        node_id: None,
        display_name,
        host,
        port,
        username,
        auth_method,
        secret,
        agent_endpoint_url: final_agent_url,
        workdir: workdir.filter(|w| !w.is_empty()),
        bind_to_bot: true,
    })
}

/// Redacts passwords, bearer/agent tokens, JSON secret fields, and PEM private key blocks.
pub fn redact_cloud_secrets(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // 1. Redact PEM private key blocks.
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(begin_idx) = rest.find("-----BEGIN ") {
        output.push_str(&rest[..begin_idx]);
        let after_begin = &rest[begin_idx..];
        if let Some(header_end) = after_begin.find("-----") {
            let after_first_marker = &after_begin[header_end + 5..];
            if let Some(second_marker_end) = after_first_marker.find("-----") {
                let header = &after_begin[..header_end + 5 + second_marker_end + 5];
                if header.contains("PRIVATE KEY") {
                    if let Some(end_idx) = after_begin.find("-----END ") {
                        let tail = &after_begin[end_idx..];
                        if let Some(close_rel) = tail[9..].find("-----") {
                            let end_total = end_idx + 9 + close_rel + 5;
                            output.push_str("[REDACTED_PRIVATE_KEY]");
                            rest = &after_begin[end_total..];
                            continue;
                        }
                    }
                    output.push_str("[REDACTED_PRIVATE_KEY]");
                    rest = "";
                    break;
                }
            }
        }
        output.push_str("-----BEGIN ");
        rest = &rest[begin_idx + 11..];
    }
    output.push_str(rest);

    // 2. Redact JSON secret string fields (`"secret": "..."`, `"password": "..."`, `"token": "..."`, etc.).
    output = redact_json_secret_fields(&output);

    // 3. Redact key-value secrets in text (`password: foo`, `密码：foo`, `token=foo`, `sshpass -p foo`, `Bearer tok`).
    output = redact_inline_secret_values(&output);

    output
}

fn extract_pem_private_key(input: &str) -> (Option<String>, String) {
    let mut search_from = 0usize;
    while let Some(rel_begin) = input[search_from..].find("-----BEGIN ") {
        let begin_idx = search_from + rel_begin;
        let after_begin = &input[begin_idx..];
        let Some(first_newline_or_end) = after_begin.find("PRIVATE KEY-----") else {
            search_from = begin_idx + 11;
            continue;
        };
        let header_end = begin_idx + first_newline_or_end + "PRIVATE KEY-----".len();
        if let Some(rel_end_tag) = input[header_end..].find("-----END ") {
            let end_tag_start = header_end + rel_end_tag;
            if let Some(rel_close) = input[end_tag_start..].find("PRIVATE KEY-----") {
                let end_idx = end_tag_start + rel_close + "PRIVATE KEY-----".len();
                let pem_block = input[begin_idx..end_idx].trim().to_string();
                let inner_body = input[header_end..end_tag_start].trim();
                if !inner_body.is_empty() {
                    let mut scrubbed = String::with_capacity(input.len());
                    scrubbed.push_str(&input[..begin_idx]);
                    scrubbed.push(' ');
                    scrubbed.push_str(&input[end_idx..]);
                    return (Some(pem_block), scrubbed);
                }
            }
        }
        search_from = header_end;
    }
    (None, input.to_string())
}

fn extract_http_urls(input: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for scheme in ["https://", "http://"] {
        let mut cursor = 0usize;
        while let Some(rel) = input[cursor..].find(scheme) {
            let start = cursor + rel;
            let rest = &input[start..];
            let end = rest
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | ')' | ']' | '>' | '，' | '；' | '。')
                })
                .unwrap_or(rest.len());
            let candidate = trim_trailing_punct(&rest[..end]);
            if candidate.len() > scheme.len() && !urls.iter().any(|u| u == candidate) {
                urls.push(candidate.to_string());
            }
            cursor = start + end.max(scheme.len());
        }
    }
    urls
}

fn parse_url_host_port(url: &str) -> Option<(String, Option<u16>)> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = without_scheme.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some((h, p)) = split_host_and_port(host_port) {
        Some((h, Some(p)))
    } else {
        Some((host_port.to_string(), None))
    }
}

fn split_host_and_port(raw: &str) -> Option<(String, u16)> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        // IPv6 `[::1]:2222`
        let close = rest.find(']')?;
        let host = &rest[..close];
        let after = rest[close + 1..].strip_prefix(':')?;
        let port = after.parse::<u16>().ok()?;
        return Some((host.to_string(), port));
    }
    let (host, port_str) = trimmed.rsplit_once(':')?;
    if host.is_empty() || host.contains(':') {
        return None;
    }
    let port = port_str.parse::<u16>().ok()?;
    Some((host.to_string(), port))
}

fn extract_keyed_value(input: &str, keys: &[&str]) -> Option<String> {
    extract_keyed_value_matching(input, keys, |_| true)
}

fn extract_keyed_value_matching<F>(input: &str, keys: &[&str], mut accept: F) -> Option<String>
where
    F: FnMut(&str) -> bool,
{
    let lower = input.to_lowercase();
    for key in keys {
        let key_lower = key.to_lowercase();
        let mut search_from = 0usize;
        while let Some(rel) = lower[search_from..].find(&key_lower) {
            let start = search_from + rel;
            let end = start + key_lower.len();
            search_from = end;

            if !has_left_key_boundary(input, start) {
                continue;
            }
            if !has_right_key_boundary(input, end) {
                continue;
            }

            let after_key = &input[end..];
            if let Some(val) = parse_value_after_key(after_key) {
                if !val.is_empty() && accept(&val) {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn is_valid_host_or_url_candidate(raw: &str) -> bool {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        parse_url_host_port(raw).is_some_and(|(h, _)| is_valid_host_token(&h))
    } else if let Some((h, _)) = split_host_and_port(raw) {
        is_valid_host_token(&h)
    } else {
        is_valid_host_token(raw)
    }
}

fn has_left_key_boundary(text: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 {
        return true;
    }
    let Some(prev) = text[..byte_idx].chars().next_back() else {
        return true;
    };
    if text[byte_idx..].chars().next().is_some_and(|c| !c.is_ascii()) {
        // CJK keywords ("密码", "服务器", "地址", "端口", "用户名", "目录") frequently appear immediately
        // after punctuation or spaces or other Chinese characters like "我的服务器 1.2.3.4".
        return !prev.is_ascii_alphanumeric() && prev != '_';
    }
    !prev.is_ascii_alphanumeric() && prev != '_' && prev != '-'
}

fn has_right_key_boundary(text: &str, byte_idx: usize) -> bool {
    let Some(next) = text[byte_idx..].chars().next() else {
        return false;
    };
    next.is_whitespace() || matches!(next, ':' | '：' | '=' | '"' | '\'')
}

fn parse_value_after_key(after_key: &str) -> Option<String> {
    let mut s = after_key.trim_start_matches(|c: char| c == ' ' || c == '\t');
    let mut had_explicit_sep = false;
    if let Some(rest) = s
        .strip_prefix(':')
        .or_else(|| s.strip_prefix('：'))
        .or_else(|| s.strip_prefix('='))
    {
        had_explicit_sep = true;
        s = rest.trim_start_matches(|c: char| c == ' ' || c == '\t');
    } else if !after_key.starts_with(char::is_whitespace) {
        return None;
    }

    if s.is_empty() || s.starts_with('\n') || s.starts_with('\r') {
        return None;
    }

    // Support quoted strings (can contain spaces).
    let first = s.chars().next()?;
    if first == '"' || first == '\'' {
        let rest = &s[first.len_utf8()..];
        if let Some(close) = rest.find(first) {
            let quoted = rest[..close].trim().to_string();
            return (!quoted.is_empty()).then_some(quoted);
        }
    }

    // Skip optional filler "是" / "为" after Chinese keyword without colon only when explicit sep wasn't used.
    if !had_explicit_sep {
        if let Some(rest) = s.strip_prefix("是").or_else(|| s.strip_prefix("为")) {
            s = rest.trim_start_matches(|c: char| c == ' ' || c == '\t');
        }
    }

    // Take up to whitespace or statement separator `,` / `;` / Chinese comma/period (note: commas/semicolons
    // can appear in some passwords unless followed by space, so split at whitespace or trailing `,`/`;`).
    let mut end = s.len();
    for (idx, ch) in s.char_indices() {
        if ch.is_whitespace() || matches!(ch, '，' | '；' | '。' | '\n' | '\r') {
            end = idx;
            break;
        }
        if matches!(ch, ',' | ';') {
            // Treat comma/semicolon as a field separator unless we're inside a URL? No, URLs don't contain `,` or `;` normally.
            let after = &s[idx + ch.len_utf8()..];
            if after.is_empty() || after.starts_with(char::is_whitespace) || looks_like_next_kv(after) {
                end = idx;
                break;
            }
        }
    }

    let token = s[..end].trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

fn looks_like_next_kv(s: &str) -> bool {
    let lower = s.trim_start().to_lowercase();
    [
        "host",
        "ip",
        "port",
        "user",
        "username",
        "pass",
        "pwd",
        "password",
        "token",
        "name",
        "workdir",
        "cwd",
        "agent",
        "端口",
        "用户",
        "密码",
        "名称",
        "目录",
    ]
    .iter()
    .any(|kw| lower.starts_with(kw))
}

fn trim_trailing_punct(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(c, ',' | ';' | '"' | '\'' | ')' | ']' | '，' | '；' | '。' | '`')
    })
}

fn extract_user_at_host(input: &str) -> Option<(String, String, Option<u16>)> {
    for raw_tok in input.split_whitespace() {
        let tok = trim_trailing_punct(raw_tok);
        if tok.starts_with("http://") || tok.starts_with("https://") {
            continue;
        }
        let Some((user, host_part)) = tok.split_once('@') else {
            continue;
        };
        if user.is_empty()
            || host_part.is_empty()
            || user.contains(['/', ':', '\\'])
            || host_part.contains('/')
        {
            continue;
        }
        if let Some((host, port)) = split_host_and_port(host_part) {
            if is_valid_host_token(&host) {
                return Some((user.to_string(), host, Some(port)));
            }
        } else if is_valid_host_token(host_part) {
            return Some((user.to_string(), host_part.to_string(), None));
        }
    }
    None
}

fn extract_first_ipv4(input: &str) -> Option<(String, Option<u16>)> {
    for raw_tok in input.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '，' | '；' | '。' | '`')
    }) {
        if raw_tok.is_empty() {
            continue;
        }
        let candidate = raw_tok
            .strip_prefix("host:")
            .or_else(|| raw_tok.strip_prefix("ip:"))
            .unwrap_or(raw_tok);
        if let Some((h, p)) = split_host_and_port(candidate) {
            if is_ipv4(&h) {
                return Some((h, Some(p)));
            }
        } else if is_ipv4(candidate) {
            return Some((candidate.to_string(), None));
        }
    }
    None
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.len() <= 3 && part.parse::<u8>().is_ok())
}

fn is_valid_host_token(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || host.contains(char::is_whitespace)
        || host.contains(['?', '/', '\\', '"', '\'', '<', '>'])
    {
        return false;
    }
    if is_ipv4(host) || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // Accept hostnames like `cloud.example.com` or `prod-node-1`.
    host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !host.starts_with('.')
        && !host.ends_with('.')
}

fn is_empty_or_placeholder_secret(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.is_empty()
        || trimmed == "?"
        || trimmed == "？"
        || trimmed == "..."
        || trimmed.eq_ignore_ascii_case("null")
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed == "[REDACTED]"
}

fn prompt_contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn redact_json_secret_fields(text: &str) -> String {
    const JSON_SECRET_KEYS: &[&str] = &[
        "\"secret\"",
        "\"password\"",
        "\"passwd\"",
        "\"token\"",
        "\"access_token\"",
        "\"agent_token\"",
        "\"private_key\"",
        "\"ssh_private_key\"",
    ];

    let mut out = text.to_string();
    for key in JSON_SECRET_KEYS {
        let mut rebuilt = String::with_capacity(out.len());
        let mut cursor = 0usize;
        let lower = out.to_lowercase();
        while let Some(rel) = lower[cursor..].find(key) {
            let match_start = cursor + rel;
            let after_key_idx = match_start + key.len();
            rebuilt.push_str(&out[cursor..after_key_idx]);
            let rest = &out[after_key_idx..];
            let ws_len = rest.len() - rest.trim_start().len();
            let after_ws = &rest[ws_len..];
            if let Some(after_colon) = after_ws.strip_prefix(':') {
                let ws2_len = after_colon.len() - after_colon.trim_start().len();
                let val_start_slice = &after_colon[ws2_len..];
                if let Some(after_open_quote) = val_start_slice.strip_prefix('"') {
                    // Find closing quote accounting for backslash escapes.
                    let mut escaped = false;
                    let mut close_idx = None;
                    for (idx, ch) in after_open_quote.char_indices() {
                        if escaped {
                            escaped = false;
                            continue;
                        }
                        if ch == '\\' {
                            escaped = true;
                            continue;
                        }
                        if ch == '"' {
                            close_idx = Some(idx);
                            break;
                        }
                    }
                    if let Some(ci) = close_idx {
                        let prefix_len = ws_len + 1 + ws2_len + 1;
                        rebuilt.push_str(&rest[..prefix_len]);
                        rebuilt.push_str("[REDACTED]");
                        let consumed = prefix_len + ci;
                        cursor = after_key_idx + consumed;
                        continue;
                    }
                }
            }
            cursor = after_key_idx;
        }
        rebuilt.push_str(&out[cursor..]);
        out = rebuilt;
    }
    out
}

fn redact_inline_secret_values(text: &str) -> String {
    const INLINE_KEYS: &[&str] = &[
        "password",
        "passwd",
        "pwd",
        "pass",
        "agent_token",
        "access_token",
        "token",
        "secret",
        "密码",
        "Bearer",
    ];

    let mut out = text.to_string();
    for key in INLINE_KEYS {
        let mut rebuilt = String::with_capacity(out.len());
        let mut cursor = 0usize;
        let lower = out.to_lowercase();
        let key_lower = key.to_lowercase();

        while let Some(rel) = lower[cursor..].find(&key_lower) {
            let start = cursor + rel;
            let end = start + key_lower.len();
            if !has_left_key_boundary(&out, start) {
                rebuilt.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            }

            // Do not match identifiers like `secret_redacted` or `token_count`.
            if let Some(next_ch) = out[end..].chars().next() {
                if next_ch == '_' || next_ch.is_ascii_alphanumeric() {
                    rebuilt.push_str(&out[cursor..end]);
                    cursor = end;
                    continue;
                }
            }

            // Skip JSON-quoted `"secret":` (already handled cleanly by `redact_json_secret_fields`).
            if out[..start].ends_with('"') && out[end..].starts_with('"') {
                rebuilt.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            }

            let rest = &out[end..];
            // Require whitespace, `:`, `：`, or `=` directly after the keyword.
            let Some(first_after) = rest.chars().next() else {
                rebuilt.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            };
            if !first_after.is_whitespace() && !matches!(first_after, ':' | '：' | '=') {
                rebuilt.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            }

            // Scan separator (`:`, `：`, `=`, spaces/tabs).
            let mut sep_bytes = 0usize;
            for ch in rest.chars() {
                if ch == ' ' || ch == '\t' || matches!(ch, ':' | '：' | '=') {
                    sep_bytes += ch.len_utf8();
                } else {
                    break;
                }
            }

            let val_slice = &rest[sep_bytes..];
            if val_slice.is_empty() || val_slice.starts_with(['\n', '\r']) {
                rebuilt.push_str(&out[cursor..end]);
                cursor = end;
                continue;
            }

            // Find end of value token (quoted or unquoted).
            let val_len = if let Some(q) = val_slice.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let inner = &val_slice[1..];
                if let Some(close) = inner.find(q) {
                    1 + close + 1
                } else {
                    val_slice
                        .find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '，' | '；' | '。'))
                        .unwrap_or(val_slice.len())
                }
            } else {
                val_slice
                    .find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '，' | '；' | '。'))
                    .unwrap_or(val_slice.len())
            };

            let val_raw = &val_slice[..val_len];
            let trimmed_val = val_raw.trim_matches(|c| c == '"' || c == '\'');
            if trimmed_val.is_empty()
                || trimmed_val == "[REDACTED]"
                || trimmed_val == "[REDACTED_SECRET]"
                || trimmed_val.eq_ignore_ascii_case("true")
                || trimmed_val.eq_ignore_ascii_case("false")
                || trimmed_val.eq_ignore_ascii_case("redacted")
            {
                rebuilt.push_str(&out[cursor..end + sep_bytes + val_len]);
                cursor = end + sep_bytes + val_len;
                continue;
            }

            rebuilt.push_str(&out[cursor..end + sep_bytes]);
            rebuilt.push_str("[REDACTED]");
            cursor = end + sep_bytes + val_len;
        }
        rebuilt.push_str(&out[cursor..]);
        out = rebuilt;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_password_chinese_and_english_prompts() {
        let cn = "帮我连接云服务器 地址 192.168.10.20 端口 2222 用户名 ubuntu 密码 MyS3cr3t! 工作目录 /srv/app 名称 生产节点";
        let cfg = parse_cloud_credentials_from_prompt(cn).expect("should parse chinese cloud prompt");
        assert_eq!(cfg.host, "192.168.10.20");
        assert_eq!(cfg.port, 2222);
        assert_eq!(cfg.username, "ubuntu");
        assert_eq!(cfg.auth_method, "ssh_password");
        assert_eq!(cfg.secret, "MyS3cr3t!");
        assert_eq!(cfg.workdir.as_deref(), Some("/srv/app"));
        assert_eq!(cfg.display_name, "生产节点");
        assert!(cfg.bind_to_bot);

        let en = "configure cloud server deploy@cloud.example.com:2200 pass=\"top secret\"";
        let cfg_en = parse_cloud_credentials_from_prompt(en).expect("should parse user@host");
        assert_eq!(cfg_en.host, "cloud.example.com");
        assert_eq!(cfg_en.port, 2200);
        assert_eq!(cfg_en.username, "deploy");
        assert_eq!(cfg_en.auth_method, "ssh_password");
        assert_eq!(cfg_en.secret, "top secret");
        assert_eq!(cfg_en.display_name, "Cloud-cloud.example.com");
    }

    #[test]
    fn parse_agent_http_token_and_pem_private_key() {
        let http_prompt = "agent_url: http://10.0.0.8:9090/agent token: tok_live_98765";
        let cfg = parse_cloud_credentials_from_prompt(http_prompt).expect("should parse http token");
        assert_eq!(cfg.host, "10.0.0.8");
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.auth_method, "agent_http_token");
        assert_eq!(cfg.secret, "tok_live_98765");
        assert_eq!(
            cfg.agent_endpoint_url.as_deref(),
            Some("http://10.0.0.8:9090/agent")
        );

        let pem_prompt = "host: 172.16.0.5 user: ops\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA_KEY_DATA_BBBB\n-----END OPENSSH PRIVATE KEY-----";
        let cfg_pem = parse_cloud_credentials_from_prompt(pem_prompt).expect("should parse pem");
        assert_eq!(cfg_pem.host, "172.16.0.5");
        assert_eq!(cfg_pem.port, 22);
        assert_eq!(cfg_pem.username, "ops");
        assert_eq!(cfg_pem.auth_method, "ssh_private_key");
        assert!(cfg_pem.secret.contains("BEGIN OPENSSH PRIVATE KEY"));
    }

    #[test]
    fn parse_rejects_normal_prompts_without_host_or_secret() {
        assert!(parse_cloud_credentials_from_prompt("帮我看下我有哪些电脑").is_none());
        assert!(parse_cloud_credentials_from_prompt("check status on 192.168.1.50").is_none());
        assert!(parse_cloud_credentials_from_prompt("how do I change my password?").is_none());
        assert!(parse_cloud_credentials_from_prompt("list all local computers").is_none());
    }

    #[test]
    fn redact_cloud_secrets_masks_credentials_without_breaking_flags() {
        let json_msg = r#"{"node_id":"cloud:1","secret":"SuperSecret99","secret_redacted":true}"#;
        let redacted_json = redact_cloud_secrets(json_msg);
        assert!(!redacted_json.contains("SuperSecret99"));
        assert!(redacted_json.contains("\"secret\":\"[REDACTED]\"") || redacted_json.contains("\"secret\": \"[REDACTED]\"") || redacted_json.contains("\"[REDACTED]\""));
        assert!(redacted_json.contains("\"secret_redacted\":true"));

        let text_msg = "host=1.2.3.4 password=MyPass123 token=tok_xyz\n-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----";
        let redacted = redact_cloud_secrets(text_msg);
        assert!(!redacted.contains("MyPass123"));
        assert!(!redacted.contains("tok_xyz"));
        assert!(!redacted.contains("\nabc\n"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("[REDACTED_PRIVATE_KEY]"));
    }
}
