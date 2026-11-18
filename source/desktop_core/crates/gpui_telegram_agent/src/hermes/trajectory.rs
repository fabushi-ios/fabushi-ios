use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryFormat {
    ShareGpt,
    OpenAiJsonl,
    Anthropic,
    HermesNative,
}

impl TrajectoryFormat {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "sharegpt" | "share_gpt" => Some(Self::ShareGpt),
            "openai" | "openai_jsonl" | "jsonl" | "chatml" => Some(Self::OpenAiJsonl),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "hermes" | "hermes_native" | "native" | "json" => Some(Self::HermesNative),
            _ => None,
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::OpenAiJsonl => "jsonl",
            Self::ShareGpt | Self::Anthropic | Self::HermesNative => "json",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrajectoryToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryMessage {
    pub seq: i64,
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<TrajectoryToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub token_count: usize,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectorySession {
    pub session_id: String,
    pub chat_id: i64,
    pub bot_id: i64,
    pub model: String,
    pub api_mode: String,
    pub title: String,
    #[serde(default)]
    pub system_prompt: String,
    pub messages: Vec<TrajectoryMessage>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_cost_usd: f64,
    #[serde(default)]
    pub metadata: Value,
}

// ============================================================================
// Secret Redactor
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionReport {
    pub total_redactions: usize,
    pub by_category: BTreeMap<String, usize>,
}

impl RedactionReport {
    pub fn record(&mut self, category: &str) {
        self.total_redactions += 1;
        *self.by_category.entry(category.to_string()).or_insert(0) += 1;
    }

    pub fn merge(&mut self, other: &RedactionReport) {
        self.total_redactions += other.total_redactions;
        for (k, v) in &other.by_category {
            *self.by_category.entry(k.clone()).or_insert(0) += v;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SecretRedactor {
    custom_literals: Vec<String>,
}

impl SecretRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_custom_secret(mut self, secret: impl Into<String>) -> Self {
        let s = secret.into();
        if !s.trim().is_empty() {
            self.custom_literals.push(s);
        }
        self
    }

    pub fn redact_text(&self, input: &str) -> (String, RedactionReport) {
        let mut report = RedactionReport::default();
        let mut text = input.to_string();

        // 1. Custom exact literals
        for lit in &self.custom_literals {
            if text.contains(lit.as_str()) {
                let count = text.matches(lit.as_str()).count();
                text = text.replace(lit.as_str(), "[REDACTED:CUSTOM_SECRET]");
                for _ in 0..count {
                    report.record("custom_secret");
                }
            }
        }

        // 2. PEM private key blocks
        text = redact_pem_blocks(&text, &mut report);

        // 3. Bearer / Basic auth headers
        text = redact_auth_scheme(&text, "Bearer ", "[REDACTED:BEARER_TOKEN]", "bearer_token", &mut report);
        text = redact_auth_scheme(&text, "bearer ", "[REDACTED:BEARER_TOKEN]", "bearer_token", &mut report);
        text = redact_auth_scheme(&text, "Basic ", "[REDACTED:BASIC_AUTH]", "basic_auth", &mut report);

        // 4. Prefixed secret tokens (`sk-`, `ghp_`, `xoxb-`, `AIza`, `AKIA`)
        text = redact_prefixed_tokens(&text, &mut report);

        // 5. Key-value credential assignments (`api_key=...`, `aws_secret_access_key=...`, `x-api-key: ...`)
        text = redact_key_value_secrets(&text, &mut report);

        (text, report)
    }

    pub fn redact_session(&self, session: &TrajectorySession) -> (TrajectorySession, RedactionReport) {
        let mut total = RedactionReport::default();
        let mut cloned = session.clone();

        let (sys, r_sys) = self.redact_text(&cloned.system_prompt);
        cloned.system_prompt = sys;
        total.merge(&r_sys);

        for msg in &mut cloned.messages {
            let (content, r_c) = self.redact_text(&msg.content);
            msg.content = content;
            total.merge(&r_c);

            if let Some(reason) = &msg.reasoning {
                let (r_text, r_r) = self.redact_text(reason);
                msg.reasoning = Some(r_text);
                total.merge(&r_r);
            }
            for tc in &mut msg.tool_calls {
                let (args, r_a) = self.redact_text(&tc.arguments);
                tc.arguments = args;
                total.merge(&r_a);
            }
        }
        (cloned, total)
    }
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '=' | '+' | '/')
}

fn redact_pem_blocks(input: &str, report: &mut RedactionReport) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("-----BEGIN ") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start..];
        if let Some(end_marker_pos) = after_start.find("-----END ") {
            let tail = &after_start[end_marker_pos..];
            if let Some(end_dashes) = tail[9..].find("-----") {
                let full_end = end_marker_pos + 9 + end_dashes + 5;
                out.push_str("[REDACTED:PRIVATE_KEY]");
                report.record("private_key");
                rest = &after_start[full_end..];
                continue;
            }
        }
        out.push_str("-----BEGIN ");
        rest = &rest[start + 11..];
    }
    out.push_str(rest);
    out
}

fn redact_auth_scheme(
    input: &str,
    prefix: &str,
    replacement: &str,
    category: &str,
    report: &mut RedactionReport,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find(prefix) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + prefix.len()..];
        let tok_len = after
            .char_indices()
            .take_while(|(_, c)| is_token_char(*c))
            .map(|(idx, c)| idx + c.len_utf8())
            .last()
            .unwrap_or(0);
        let token = &after[..tok_len];
        if tok_len >= 8 && !token.starts_with("[REDACTED") {
            out.push_str(prefix);
            out.push_str(replacement);
            report.record(category);
            rest = &after[tok_len..];
        } else {
            out.push_str(prefix);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn redact_prefixed_tokens(input: &str, report: &mut RedactionReport) -> String {
    const PREFIXES: &[(&str, usize, &str, &str)] = &[
        ("sk-ant-", 16, "[REDACTED:ANTHROPIC_KEY]", "anthropic_key"),
        ("sk-proj-", 16, "[REDACTED:OPENAI_KEY]", "openai_key"),
        ("sk-", 18, "[REDACTED:API_KEY]", "api_key"),
        ("github_pat_", 18, "[REDACTED:GITHUB_TOKEN]", "github_token"),
        ("ghp_", 16, "[REDACTED:GITHUB_TOKEN]", "github_token"),
        ("gho_", 16, "[REDACTED:GITHUB_TOKEN]", "github_token"),
        ("ghs_", 16, "[REDACTED:GITHUB_TOKEN]", "github_token"),
        ("ghu_", 16, "[REDACTED:GITHUB_TOKEN]", "github_token"),
        ("xoxb-", 14, "[REDACTED:SLACK_TOKEN]", "slack_token"),
        ("xoxp-", 14, "[REDACTED:SLACK_TOKEN]", "slack_token"),
        ("xoxa-", 14, "[REDACTED:SLACK_TOKEN]", "slack_token"),
        ("xoxr-", 14, "[REDACTED:SLACK_TOKEN]", "slack_token"),
        ("AIza", 24, "[REDACTED:GOOGLE_API_KEY]", "google_api_key"),
        ("AKIA", 18, "[REDACTED:AWS_ACCESS_KEY]", "aws_access_key"),
        ("ASIA", 18, "[REDACTED:AWS_ACCESS_KEY]", "aws_access_key"),
    ];

    let mut out = String::with_capacity(input.len());
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let byte_idx = chars[i].0;
        let slice = &input[byte_idx..];
        let prev_ok = if i == 0 {
            true
        } else {
            let p = chars[i - 1].1;
            !p.is_ascii_alphanumeric() && p != '_'
        };

        let mut matched = false;
        if prev_ok {
            for (prefix, min_total_len, marker, category) in PREFIXES {
                if slice.starts_with(prefix) {
                    let tok_bytes = slice
                        .char_indices()
                        .take_while(|(_, c)| is_token_char(*c) && *c != '=')
                        .map(|(idx, c)| idx + c.len_utf8())
                        .last()
                        .unwrap_or(0);
                    if tok_bytes >= *min_total_len {
                        out.push_str(marker);
                        report.record(category);
                        let target_byte = byte_idx + tok_bytes;
                        while i < chars.len() && chars[i].0 < target_byte {
                            i += 1;
                        }
                        matched = true;
                        break;
                    }
                }
            }
        }
        if !matched {
            out.push(chars[i].1);
            i += 1;
        }
    }
    out
}

fn redact_key_value_secrets(input: &str, report: &mut RedactionReport) -> String {
    const KEYS: &[&str] = &[
        "aws_secret_access_key",
        "openai_api_key",
        "anthropic_api_key",
        "openrouter_api_key",
        "x-api-key",
        "api_key",
        "apikey",
        "access_token",
        "secret_key",
        "client_secret",
        "password",
    ];

    let mut lines_out = Vec::new();
    for line in input.split('\n') {
        let lower = line.to_ascii_lowercase();
        let mut modified = line.to_string();
        for key in KEYS {
            if let Some(pos) = lower.find(key) {
                let after_key = &modified[pos + key.len()..];
                let trimmed = after_key.trim_start();
                if let Some(sep) = trimmed.chars().next() {
                    if sep == '=' || sep == ':' {
                        let sep_offset = after_key.len() - trimmed.len() + sep.len_utf8();
                        let val_slice = &after_key[sep_offset..];
                        let ws_len = val_slice.len() - val_slice.trim_start().len();
                        let val_start = pos + key.len() + sep_offset + ws_len;
                        let rest = &modified[val_start..];
                        let (quote_prefix, actual_rest) = if let Some(r) = rest.strip_prefix('"') {
                            ("\"", r)
                        } else if let Some(r) = rest.strip_prefix('\'') {
                            ("'", r)
                        } else {
                            ("", rest)
                        };
                        let val_len = actual_rest
                            .char_indices()
                            .take_while(|(_, c)| {
                                !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ',' && *c != ';' && *c != '}'
                            })
                            .map(|(idx, c)| idx + c.len_utf8())
                            .last()
                            .unwrap_or(0);
                        let candidate = &actual_rest[..val_len];
                        if val_len >= 6 && !candidate.starts_with("[REDACTED") {
                            let end_idx = val_start + quote_prefix.len() + val_len;
                            let mut rebuilt = String::new();
                            rebuilt.push_str(&modified[..val_start + quote_prefix.len()]);
                            rebuilt.push_str("[REDACTED:SECRET]");
                            rebuilt.push_str(&modified[end_idx..]);
                            modified = rebuilt;
                            report.record("key_value_secret");
                            break;
                        }
                    }
                }
            }
        }
        lines_out.push(modified);
    }
    lines_out.join("\n")
}

// ============================================================================
// Trajectory Compressor / Filter
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryCompressionConfig {
    pub max_tokens: usize,
    pub deduplicate_tool_retries: bool,
    pub drop_empty_turns: bool,
    pub min_turns: usize,
    pub head_preserve_turns: usize,
    pub tail_preserve_turns: usize,
}

impl Default for TrajectoryCompressionConfig {
    fn default() -> Self {
        Self {
            max_tokens: 16_000,
            deduplicate_tool_retries: true,
            drop_empty_turns: true,
            min_turns: 2,
            head_preserve_turns: 2,
            tail_preserve_turns: 4,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionStats {
    pub original_messages: usize,
    pub kept_messages: usize,
    pub deduplicated_retries: usize,
    pub dropped_middle_turns: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub passed_quality_filter: bool,
}

pub struct TrajectoryCompressor;

impl TrajectoryCompressor {
    pub fn estimate_message_tokens(msg: &TrajectoryMessage) -> usize {
        if msg.token_count > 0 {
            return msg.token_count;
        }
        let tool_chars: usize = msg
            .tool_calls
            .iter()
            .map(|tc| tc.name.len() + tc.arguments.len() + 12)
            .sum();
        let total_chars =
            msg.content.chars().count() + msg.reasoning.as_deref().map(|r| r.chars().count()).unwrap_or(0) + tool_chars;
        ((total_chars as f64) / 3.5).ceil().max(1.0) as usize
    }

    pub fn compress(
        session: &TrajectorySession,
        config: &TrajectoryCompressionConfig,
    ) -> (TrajectorySession, CompressionStats) {
        let mut out = session.clone();
        let original_messages = out.messages.len();
        let estimated_tokens_before: usize = out
            .messages
            .iter()
            .map(Self::estimate_message_tokens)
            .sum();

        let mut messages = out.messages;

        // 1. Optional empty-turn cleanup (keep assistant messages that have tool_calls!)
        if config.drop_empty_turns {
            messages.retain(|m| {
                !m.content.trim().is_empty()
                    || !m.tool_calls.is_empty()
                    || m.reasoning.as_deref().is_some_and(|r| !r.trim().is_empty())
            });
        }

        // 2. Deduplicate consecutive noisy identical tool retries while keeping pairing intact
        let mut deduplicated_retries = 0usize;
        if config.deduplicate_tool_retries && messages.len() >= 4 {
            messages = Self::dedup_tool_retry_sequences(messages, &mut deduplicated_retries);
        }

        // 3. Budget fitting: drop middle non-protected turns cleanly while preserving tool-call groups
        let mut dropped_middle_turns = 0usize;
        let current_tokens = |msgs: &[TrajectoryMessage]| -> usize {
            msgs.iter().map(Self::estimate_message_tokens).sum()
        };

        if config.max_tokens > 0 && current_tokens(&messages) > config.max_tokens {
            let head_keep = config.head_preserve_turns.min(messages.len());
            let tail_keep = config.tail_preserve_turns.min(messages.len().saturating_sub(head_keep));

            while current_tokens(&messages) > config.max_tokens
                && messages.len() > head_keep + tail_keep
            {
                let removable_idx = head_keep;
                // Identify atomic group: if removable_idx is an assistant w/ tool_calls, drop its paired tool results too
                let group_end = Self::tool_group_end(&messages, removable_idx);
                if group_end >= messages.len().saturating_sub(tail_keep) {
                    break;
                }
                let removed_count = group_end - removable_idx + 1;
                messages.drain(removable_idx..=group_end);
                dropped_middle_turns += removed_count;
            }
        }

        // 4. Repair any orphaned tool results or dangling tool_calls
        messages = Self::repair_tool_pairing(messages);

        let estimated_tokens_after = current_tokens(&messages);
        let kept_messages = messages.len();
        let has_user = messages.iter().any(|m| m.role == "user");
        let has_assistant = messages.iter().any(|m| m.role == "assistant");
        let passed_quality_filter = kept_messages >= config.min_turns && has_user && has_assistant;

        out.messages = messages;
        let stats = CompressionStats {
            original_messages,
            kept_messages,
            deduplicated_retries,
            dropped_middle_turns,
            estimated_tokens_before,
            estimated_tokens_after,
            passed_quality_filter,
        };
        (out, stats)
    }

    fn tool_group_end(messages: &[TrajectoryMessage], start: usize) -> usize {
        if messages[start].role == "assistant" && !messages[start].tool_calls.is_empty() {
            let ids: HashSet<&str> = messages[start]
                .tool_calls
                .iter()
                .map(|tc| tc.id.as_str())
                .collect();
            let mut end = start;
            while end + 1 < messages.len()
                && messages[end + 1].role == "tool"
                && messages[end + 1]
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| ids.contains(id))
            {
                end += 1;
            }
            end
        } else {
            start
        }
    }

    fn dedup_tool_retry_sequences(
        messages: Vec<TrajectoryMessage>,
        dedup_count: &mut usize,
    ) -> Vec<TrajectoryMessage> {
        // Group messages into atomic turns: (assistant w/ tool_calls + following tool results) or single message
        let mut groups: Vec<Vec<TrajectoryMessage>> = Vec::new();
        let mut i = 0;
        while i < messages.len() {
            let end = Self::tool_group_end(&messages, i);
            groups.push(messages[i..=end].to_vec());
            i = end + 1;
        }

        let group_signature = |g: &[TrajectoryMessage]| -> Option<String> {
            let first = g.first()?;
            if first.role != "assistant" || first.tool_calls.is_empty() {
                return None;
            }
            let tc_sig = first
                .tool_calls
                .iter()
                .map(|tc| format!("{}:{}", tc.name, tc.arguments))
                .collect::<Vec<_>>()
                .join("|");
            let res_sig = g[1..]
                .iter()
                .map(|m| format!("{}:{}", m.is_error, m.content.trim()))
                .collect::<Vec<_>>()
                .join("|");
            Some(format!("{tc_sig}=>{res_sig}"))
        };

        let mut kept: Vec<Vec<TrajectoryMessage>> = Vec::with_capacity(groups.len());
        let mut prev_sig: Option<String> = None;
        for group in groups {
            let sig = group_signature(&group);
            if sig.is_some() && sig == prev_sig {
                *dedup_count += group.len();
                continue;
            }
            prev_sig = sig;
            kept.push(group);
        }

        kept.into_iter().flatten().collect()
    }

    fn repair_tool_pairing(messages: Vec<TrajectoryMessage>) -> Vec<TrajectoryMessage> {
        let mut emitted_call_ids: HashSet<String> = HashSet::new();
        let mut valid_result_ids: HashSet<String> = HashSet::new();
        for msg in &messages {
            if msg.role == "tool" {
                if let Some(id) = &msg.tool_call_id {
                    valid_result_ids.insert(id.clone());
                }
            }
        }

        let mut result = Vec::with_capacity(messages.len());
        for mut msg in messages {
            if msg.role == "assistant" && !msg.tool_calls.is_empty() {
                msg.tool_calls
                    .retain(|tc| valid_result_ids.contains(&tc.id));
                for tc in &msg.tool_calls {
                    emitted_call_ids.insert(tc.id.clone());
                }
                if msg.content.trim().is_empty() && msg.tool_calls.is_empty() {
                    continue;
                }
            } else if msg.role == "tool" {
                let paired = msg
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| emitted_call_ids.contains(id));
                if !paired {
                    continue;
                }
            }
            result.push(msg);
        }
        result
    }
}

// ============================================================================
// Trajectory Exporter
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedTrajectory {
    pub session_id: String,
    pub format: TrajectoryFormat,
    pub payload: String,
    pub message_count: usize,
    pub redaction_report: RedactionReport,
    pub compression_stats: Option<CompressionStats>,
}

pub struct TrajectoryExporter;

impl TrajectoryExporter {
    pub fn export_session(
        session: &TrajectorySession,
        format: TrajectoryFormat,
        redact: bool,
        compression: Option<&TrajectoryCompressionConfig>,
    ) -> Result<ExportedTrajectory, String> {
        let (mut working, redaction_report) = if redact {
            SecretRedactor::new().redact_session(session)
        } else {
            (session.clone(), RedactionReport::default())
        };

        let compression_stats = if let Some(cfg) = compression {
            let (compressed, stats) = TrajectoryCompressor::compress(&working, cfg);
            working = compressed;
            Some(stats)
        } else {
            None
        };

        let payload = match format {
            TrajectoryFormat::ShareGpt => Self::format_sharegpt(&working)?,
            TrajectoryFormat::OpenAiJsonl => Self::format_openai_line(&working)?,
            TrajectoryFormat::Anthropic => Self::format_anthropic(&working)?,
            TrajectoryFormat::HermesNative => serde_json::to_string_pretty(&working)
                .map_err(|e| format!("Failed to serialize HermesNative trajectory: {e}"))?,
        };

        Ok(ExportedTrajectory {
            session_id: working.session_id.clone(),
            format,
            payload,
            message_count: working.messages.len(),
            redaction_report,
            compression_stats,
        })
    }

    pub fn export_batch(
        sessions: &[TrajectorySession],
        format: TrajectoryFormat,
        redact: bool,
        compression: Option<&TrajectoryCompressionConfig>,
    ) -> Result<String, String> {
        if format == TrajectoryFormat::OpenAiJsonl {
            let mut lines = Vec::with_capacity(sessions.len());
            for s in sessions {
                let item = Self::export_session(s, format, redact, compression)?;
                lines.push(item.payload);
            }
            return Ok(lines.join("\n"));
        }

        let mut items = Vec::with_capacity(sessions.len());
        for s in sessions {
            let item = Self::export_session(s, format, redact, compression)?;
            let parsed: Value = serde_json::from_str(&item.payload)
                .map_err(|e| format!("Re-parse exported payload failed: {e}"))?;
            items.push(parsed);
        }
        serde_json::to_string_pretty(&items)
            .map_err(|e| format!("Serialize trajectory batch failed: {e}"))
    }

    fn format_sharegpt(session: &TrajectorySession) -> Result<String, String> {
        let mut conversations = Vec::new();
        if !session.system_prompt.trim().is_empty() {
            conversations.push(json!({
                "from": "system",
                "value": session.system_prompt,
            }));
        }

        for msg in &session.messages {
            match msg.role.as_str() {
                "system" => {
                    conversations.push(json!({
                        "from": "system",
                        "value": msg.content,
                    }));
                }
                "user" => {
                    conversations.push(json!({
                        "from": "human",
                        "value": msg.content,
                    }));
                }
                "assistant" => {
                    let mut val = String::new();
                    if let Some(reason) = &msg.reasoning {
                        if !reason.trim().is_empty() {
                            val.push_str(&format!("<think>\n{}\n</think>\n", reason.trim()));
                        }
                    }
                    if !msg.content.is_empty() {
                        val.push_str(&msg.content);
                    }
                    for tc in &msg.tool_calls {
                        if !val.is_empty() {
                            val.push('\n');
                        }
                        val.push_str(&format!(
                            "<tool_call>\n{}\n</tool_call>",
                            json!({
                                "id": tc.id,
                                "name": tc.name,
                                "arguments": serde_json::from_str::<Value>(&tc.arguments).unwrap_or_else(|_| Value::String(tc.arguments.clone())),
                            })
                        ));
                    }
                    conversations.push(json!({
                        "from": "gpt",
                        "value": val,
                    }));
                }
                "tool" => {
                    conversations.push(json!({
                        "from": "tool",
                        "value": msg.content,
                        "tool_name": msg.tool_name,
                        "tool_call_id": msg.tool_call_id,
                    }));
                }
                other => {
                    conversations.push(json!({
                        "from": other,
                        "value": msg.content,
                    }));
                }
            }
        }

        let obj = json!({
            "id": session.session_id,
            "model": session.model,
            "conversations": conversations,
            "metadata": {
                "chat_id": session.chat_id,
                "bot_id": session.bot_id,
                "title": session.title,
                "api_mode": session.api_mode,
                "input_tokens": session.input_tokens,
                "output_tokens": session.output_tokens,
                "total_cost_usd": session.total_cost_usd,
            }
        });
        serde_json::to_string_pretty(&obj)
            .map_err(|e| format!("Failed to serialize ShareGPT trajectory: {e}"))
    }

    fn format_openai_line(session: &TrajectorySession) -> Result<String, String> {
        let mut msgs = Vec::new();
        if !session.system_prompt.trim().is_empty() {
            msgs.push(json!({
                "role": "system",
                "content": session.system_prompt,
            }));
        }
        for msg in &session.messages {
            match msg.role.as_str() {
                "assistant" => {
                    let mut obj = json!({
                        "role": "assistant",
                        "content": msg.content,
                    });
                    if !msg.tool_calls.is_empty() {
                        obj["tool_calls"] = Value::Array(
                            msg.tool_calls
                                .iter()
                                .map(|tc| {
                                    json!({
                                        "id": tc.id,
                                        "type": "function",
                                        "function": {
                                            "name": tc.name,
                                            "arguments": tc.arguments,
                                        }
                                    })
                                })
                                .collect(),
                        );
                    }
                    msgs.push(obj);
                }
                "tool" => {
                    msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": msg.tool_call_id,
                        "name": msg.tool_name,
                        "content": msg.content,
                    }));
                }
                other => {
                    msgs.push(json!({
                        "role": other,
                        "content": msg.content,
                    }));
                }
            }
        }

        let obj = json!({
            "id": session.session_id,
            "model": session.model,
            "messages": msgs,
        });
        serde_json::to_string(&obj)
            .map_err(|e| format!("Failed to serialize OpenAI JSONL line: {e}"))
    }

    fn format_anthropic(session: &TrajectorySession) -> Result<String, String> {
        let mut msgs: Vec<Value> = Vec::new();
        for msg in &session.messages {
            match msg.role.as_str() {
                "user" => {
                    msgs.push(json!({
                        "role": "user",
                        "content": [{ "type": "text", "text": msg.content }],
                    }));
                }
                "assistant" => {
                    let mut blocks = Vec::new();
                    if let Some(r) = &msg.reasoning {
                        if !r.trim().is_empty() {
                            blocks.push(json!({ "type": "thinking", "thinking": r }));
                        }
                    }
                    if !msg.content.is_empty() {
                        blocks.push(json!({ "type": "text", "text": msg.content }));
                    }
                    for tc in &msg.tool_calls {
                        let input: Value = serde_json::from_str(&tc.arguments)
                            .unwrap_or_else(|_| json!({ "raw": tc.arguments }));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": input,
                        }));
                    }
                    msgs.push(json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                }
                "tool" => {
                    let block = json!({
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                        "content": msg.content,
                        "is_error": msg.is_error,
                    });
                    if msgs.last().and_then(|v| v.get("role")).and_then(Value::as_str)
                        == Some("user")
                    {
                        if let Some(arr) = msgs
                            .last_mut()
                            .and_then(|v| v.get_mut("content"))
                            .and_then(Value::as_array_mut)
                        {
                            arr.push(block);
                            continue;
                        }
                    }
                    msgs.push(json!({
                        "role": "user",
                        "content": [block],
                    }));
                }
                _ => {}
            }
        }

        let obj = json!({
            "id": session.session_id,
            "model": session.model,
            "system": session.system_prompt,
            "messages": msgs,
        });
        serde_json::to_string_pretty(&obj)
            .map_err(|e| format!("Failed to serialize Anthropic trajectory: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_redactor_scrubs_all_common_secrets() {
        let sample = concat!(
            "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz123456\n",
            "ghp_1234567890abcdefghijklmnop\n",
            "xoxb-1234567890-abcdefghi\n",
            "Authorization: Bearer supersecretbearertoken98765\n",
            "AKIAIOSFODNN7EXAMPLE\n",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQ...\n-----END RSA PRIVATE KEY-----"
        );
        let redactor = SecretRedactor::new();
        let (scrubbed, report) = redactor.redact_text(sample);
        assert!(!scrubbed.contains("sk-proj-"));
        assert!(!scrubbed.contains("ghp_1234"));
        assert!(!scrubbed.contains("xoxb-1234"));
        assert!(!scrubbed.contains("supersecretbearertoken"));
        assert!(!scrubbed.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!scrubbed.contains("MIIEpAIBAAKCAQ"));
        assert!(report.total_redactions >= 6);
    }

    #[test]
    fn test_trajectory_compressor_dedups_retries_and_exports_sharegpt() {
        let session = TrajectorySession {
            session_id: "sess_traj_1".into(),
            chat_id: 1,
            bot_id: 777000,
            model: "hermes-local-v1".into(),
            api_mode: "local_hermes".into(),
            title: "Tool Retry Test".into(),
            system_prompt: "System prompt with secret sk-abcdefghijklmnopqrstuvwxyz123456".into(),
            messages: vec![
                TrajectoryMessage {
                    seq: 1,
                    role: "user".into(),
                    content: "Read Cargo.toml".into(),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    tool_name: None,
                    token_count: 10,
                    is_error: false,
                },
                TrajectoryMessage {
                    seq: 2,
                    role: "assistant".into(),
                    content: "".into(),
                    reasoning: None,
                    tool_calls: vec![TrajectoryToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"missing.txt"}"#.into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                    token_count: 15,
                    is_error: false,
                },
                TrajectoryMessage {
                    seq: 3,
                    role: "tool".into(),
                    content: "File not found".into(),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    tool_call_id: Some("c1".into()),
                    tool_name: Some("read_file".into()),
                    token_count: 10,
                    is_error: true,
                },
                TrajectoryMessage {
                    seq: 4,
                    role: "assistant".into(),
                    content: "".into(),
                    reasoning: None,
                    tool_calls: vec![TrajectoryToolCall {
                        id: "c2".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"missing.txt"}"#.into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                    token_count: 15,
                    is_error: false,
                },
                TrajectoryMessage {
                    seq: 5,
                    role: "tool".into(),
                    content: "File not found".into(),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    tool_call_id: Some("c2".into()),
                    tool_name: Some("read_file".into()),
                    token_count: 10,
                    is_error: true,
                },
                TrajectoryMessage {
                    seq: 6,
                    role: "assistant".into(),
                    content: "missing.txt does not exist.".into(),
                    reasoning: None,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    tool_name: None,
                    token_count: 12,
                    is_error: false,
                },
            ],
            input_tokens: 100,
            output_tokens: 50,
            total_cost_usd: 0.0,
            metadata: json!({}),
        };

        let cfg = TrajectoryCompressionConfig::default();
        let exported = TrajectoryExporter::export_session(
            &session,
            TrajectoryFormat::ShareGpt,
            true,
            Some(&cfg),
        )
        .expect("export ShareGPT");

        assert!(!exported.payload.contains("sk-abcdefgh"));
        assert!(exported.redaction_report.total_redactions >= 1);
        let stats = exported.compression_stats.unwrap();
        assert_eq!(stats.deduplicated_retries, 2);
        assert!(stats.passed_quality_filter);
        assert!(exported.payload.contains("\"from\": \"human\""));
        assert!(exported.payload.contains("\"from\": \"gpt\""));
    }
}
