use std::cmp::Ordering;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::extensions::content_search::extension::ProductionContentSearchExtension;
use crate::extensions::content_search::search_index_db::{
    AttachmentKind, MediaSearchResult, MessageSearchResult,
};
use crate::extensions::session::production::ProductionSessionWorkers;

pub const AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT: usize = 5;
pub const AGENT_CONTENT_SEARCH_MAX_RESULTS: usize = 50;
const SNIPPET_LEAD: usize = 30;
const SNIPPET_TRAIL: usize = 60;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchResult {
    pub agent_id: String,
    pub entry_id: String,
    pub role: String,
    pub timestamp_ms: f64,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterMediaSearchResult {
    pub agent_id: String,
    pub entry_id: String,
    pub file_name: String,
    pub ext: String,
    pub mime: Option<String>,
    pub kind: String,
    pub timestamp_ms: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

pub trait RosterContentSearch: Send + Sync {
    fn is_search_ready(&self) -> bool;
    fn max_matches_per_agent(&self) -> usize;
    fn max_results(&self) -> usize;
    fn search_messages(&self, query: &str, limit: usize) -> Option<Vec<MessageSearchResult>>;
    fn search_media(&self, query: &str, limit: usize) -> Option<Vec<MediaSearchResult>>;
}

impl RosterContentSearch for ProductionContentSearchExtension {
    fn is_search_ready(&self) -> bool {
        ProductionContentSearchExtension::is_search_ready(self)
    }

    fn max_matches_per_agent(&self) -> usize {
        ProductionContentSearchExtension::max_matches_per_agent(self)
    }

    fn max_results(&self) -> usize {
        ProductionContentSearchExtension::max_results(self)
    }

    fn search_messages(&self, query: &str, limit: usize) -> Option<Vec<MessageSearchResult>> {
        ProductionContentSearchExtension::search_messages(self, query, limit)
    }

    fn search_media(&self, query: &str, limit: usize) -> Option<Vec<MediaSearchResult>> {
        ProductionContentSearchExtension::search_media(self, query, limit)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentContentMatch {
    pub entry_id: String,
    pub role: String,
    pub timestamp_ms: f64,
    pub snippet: String,
}

pub fn entry_search_text(entry: &Value) -> &str {
    match entry.get("kind").and_then(Value::as_str) {
        Some("message") => entry.get("content").and_then(Value::as_str).unwrap_or_default(),
        Some("send-message")
            if entry
                .get("message")
                .and_then(Value::as_object)
                .and_then(|message| message.get("type"))
                .and_then(Value::as_str)
                == Some("text") =>
        {
            entry
                .get("message")
                .and_then(Value::as_object)
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or_default()
        }
        Some("notice") => entry.get("text").and_then(Value::as_str).unwrap_or_default(),
        _ => "",
    }
}

pub fn build_content_snippet(text: &str, normalized_query: &str) -> Option<String> {
    if normalized_query.is_empty() {
        return None;
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    let lowered = flat.to_lowercase();
    let (index, match_len) = if let Some(index) = lowered.find(normalized_query) {
        (index, normalized_query.len())
    } else if fabushi_messaging_core::query_matches_fields(normalized_query, &[&flat]) {
        fabushi_messaging_core::tokenize(normalized_query)
            .into_iter()
            .filter_map(|term| lowered.find(&term).map(|pos| (pos, term.len())))
            .min_by_key(|(pos, _)| *pos)
            .unwrap_or((0, normalized_query.len().min(flat.len())))
    } else {
        return None;
    };
    let query_end = index.saturating_add(match_len);
    let start = char_boundary_at_or_after(&flat, index.saturating_sub(SNIPPET_LEAD));
    let end = char_boundary_at_or_before(
        &flat,
        query_end.saturating_add(SNIPPET_TRAIL).min(flat.len()),
    );
    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str(ELLIPSIS);
    }
    snippet.push_str(flat.get(start..end).unwrap_or(&flat));
    if end < flat.len() {
        snippet.push_str(ELLIPSIS);
    }
    Some(snippet)
}

pub fn find_agent_content_matches(
    entries: &[Value],
    normalized_query: &str,
    limit: usize,
) -> Vec<AgentContentMatch> {
    if normalized_query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for entry in entries.iter().rev() {
        if matches.len() >= limit {
            break;
        }
        if entry
            .get("hiddenOutboundAgentPeerMessage")
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let Some(snippet) = agent_entry_search_snippet(entry, normalized_query) else {
            continue;
        };
        let Some(entry_id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let role = if entry.get("kind").and_then(Value::as_str) == Some("message") {
            entry
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("assistant")
        } else {
            "assistant"
        };
        matches.push(AgentContentMatch {
            entry_id: entry_id.to_string(),
            role: role.to_string(),
            timestamp_ms: entry
                .get("timestampMs")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or_default(),
            snippet,
        });
    }
    matches
}

fn agent_entry_search_snippet(entry: &Value, normalized_query: &str) -> Option<String> {
    let primary = entry_search_text(entry);
    if let Some(snippet) = build_content_snippet(primary, normalized_query) {
        return Some(snippet);
    }
    match entry.get("kind").and_then(Value::as_str) {
        Some("message") => {
            let mut parts = Vec::new();
            for field in ["authorName", "author", "forwardedFromAuthorName"] {
                if let Some(value) = entry
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    if !parts.contains(&value) {
                        parts.push(value);
                    }
                }
            }
            if let Some(origin_author) =
                ProductionSessionWorkers::human_entry_visible_forward_origin_author(entry)
            {
                if !parts.contains(&origin_author) {
                    parts.push(origin_author);
                }
            }
            if !primary.trim().is_empty() {
                parts.push(primary.trim());
            }
            if let Some(attachments) = entry.get("attachments").and_then(Value::as_array) {
                for attachment in attachments {
                    if let Some(name) = attachment
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        parts.push(name);
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                build_content_snippet(&parts.join(" "), normalized_query)
            }
        }
        Some("send-message") => {
            let message = entry.get("message").and_then(Value::as_object)?;
            match message.get("type").and_then(Value::as_str) {
                Some("widget") => {
                    let prompt = message
                        .get("widget")
                        .and_then(Value::as_object)
                        .and_then(|widget| widget.get("prompt"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    build_content_snippet(prompt, normalized_query)
                }
                Some("email-draft") => {
                    let draft = message.get("draft").and_then(Value::as_object)?;
                    let subject = draft
                        .get("subject")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    let body = draft
                        .get("body")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    let combined = [subject, body]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    build_content_snippet(&combined, normalized_query)
                }
                Some("slack-draft") => {
                    let body = message
                        .get("draft")
                        .and_then(Value::as_object)
                        .and_then(|draft| draft.get("body"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    build_content_snippet(body, normalized_query)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn search_human_conversation_messages(
    session: &Arc<ProductionSessionWorkers>,
    _content_search: Option<&dyn RosterContentSearch>,
    conversation_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    session.search_human_messages(conversation_id, query, limit)
}

pub fn search_agents(
    session: &Arc<ProductionSessionWorkers>,
    content_search: Option<&dyn RosterContentSearch>,
    query: &str,
    limit: usize,
) -> Result<Vec<TranscriptSearchResult>, String> {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    if let Some(content_search) = content_search.filter(|search| search.is_search_ready()) {
        let max_matches_per_agent = content_search.max_matches_per_agent();
        let indexed_limit = limit.saturating_add(max_matches_per_agent);
        if let Some(indexed) = content_search.search_messages(&normalized, indexed_limit) {
            let existing_agent_ids = session
                .list_agent_summaries(None)?
                .into_iter()
                .map(|agent| agent.id)
                .collect::<std::collections::HashSet<_>>();
            let mut results = indexed
                .into_iter()
                .filter(|matched| existing_agent_ids.contains(&matched.agent_id))
                .map(|matched| TranscriptSearchResult {
                    agent_id: matched.agent_id,
                    entry_id: matched.entry_id,
                    role: matched.role,
                    timestamp_ms: matched.timestamp_ms as f64,
                    snippet: matched.snippet,
                })
                .collect::<Vec<_>>();
            results.extend(collect_human_conversation_search_results(
                session,
                &normalized,
                max_matches_per_agent,
            )?);
            results.sort_by(|left, right| {
                right
                    .timestamp_ms
                    .partial_cmp(&left.timestamp_ms)
                    .unwrap_or(Ordering::Equal)
            });
            results.truncate(limit);
            return Ok(results);
        }
    }
    search_agents_linear(session, &normalized, limit)
}

pub fn search_media(
    session: &Arc<ProductionSessionWorkers>,
    content_search: Option<&dyn RosterContentSearch>,
    query: &str,
    limit: usize,
) -> Result<Vec<RosterMediaSearchResult>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let trimmed_query = query.trim();
    let mut results = Vec::new();
    if let Some(content_search) = content_search.filter(|search| search.is_search_ready()) {
        if let Some(indexed) = content_search.search_media(trimmed_query, limit) {
            let existing_agent_ids = session
                .list_agent_summaries(None)?
                .into_iter()
                .map(|agent| agent.id)
                .collect::<std::collections::HashSet<_>>();
            results.extend(
                indexed
                    .into_iter()
                    .filter(|matched| existing_agent_ids.contains(&matched.agent_id))
                    .map(|matched| RosterMediaSearchResult {
                        agent_id: matched.agent_id,
                        entry_id: matched.entry_id,
                        file_name: matched.file_name,
                        ext: matched.ext,
                        mime: matched.mime,
                        kind: attachment_kind_name(matched.kind).to_string(),
                        timestamp_ms: matched.timestamp_ms,
                        width: matched.width,
                        height: matched.height,
                    }),
            );
        }
    }
    results.extend(collect_human_conversation_media_results(
        session,
        trimmed_query,
        limit,
    )?);
    results.sort_by(|left, right| right.timestamp_ms.cmp(&left.timestamp_ms));
    results.truncate(limit);
    Ok(results)
}

fn attachment_kind_name(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image",
        AttachmentKind::Video => "video",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Pdf => "pdf",
        AttachmentKind::Markdown => "markdown",
        AttachmentKind::Table => "table",
        AttachmentKind::Json => "json",
        AttachmentKind::Text => "text",
        AttachmentKind::Document => "document",
        AttachmentKind::Archive => "archive",
        AttachmentKind::File => "file",
    }
}

pub fn search_agents_linear(
    session: &Arc<ProductionSessionWorkers>,
    query: &str,
    limit: usize,
) -> Result<Vec<TranscriptSearchResult>, String> {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let agents = session.list_agent_summaries(None)?;
    let mut results = Vec::new();
    for agent in agents {
        let entries = session.read_agent_transcript_entries(&agent.id)?;
        for matched in find_agent_content_matches(
            &entries,
            &normalized,
            AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT,
        ) {
            results.push(TranscriptSearchResult {
                agent_id: agent.id.clone(),
                entry_id: matched.entry_id,
                role: matched.role,
                timestamp_ms: if matched.timestamp_ms > 0.0 {
                    matched.timestamp_ms
                } else {
                    agent.updated_at
                },
                snippet: matched.snippet,
            });
        }
    }
    results.extend(collect_human_conversation_search_results(
        session,
        &normalized,
        AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT,
    )?);
    results.sort_by(|left, right| {
        right
            .timestamp_ms
            .partial_cmp(&left.timestamp_ms)
            .unwrap_or(Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

fn collect_human_conversation_search_results(
    session: &Arc<ProductionSessionWorkers>,
    normalized_query: &str,
    max_matches_per_conversation: usize,
) -> Result<Vec<TranscriptSearchResult>, String> {
    if normalized_query.is_empty() || max_matches_per_conversation == 0 {
        return Ok(Vec::new());
    }
    let Ok(local_human_id) = session.local_human_id() else {
        return Ok(Vec::new());
    };
    let conversations = session.list_human_conversations()?;
    let mut results = Vec::new();
    for conversation in conversations {
        let Some(conversation_id) = conversation.get("id").and_then(Value::as_str) else {
            continue;
        };
        let fallback_timestamp_ms = conversation
            .get("updatedAt")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or_default();
        let Ok(entries) = session.read_human_conversation_transcript(conversation_id) else {
            continue;
        };
        let mut matched_count = 0usize;
        for entry in entries.iter().rev() {
            if matched_count >= max_matches_per_conversation {
                break;
            }
            let Some(snippet) = human_entry_search_snippet(entry, normalized_query) else {
                continue;
            };
            let Some(entry_id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let role = if entry.get("authorKind").and_then(Value::as_str) == Some("human") {
                if entry.get("authorId").and_then(Value::as_str) == Some(local_human_id) {
                    "user"
                } else {
                    "assistant"
                }
            } else if entry.get("role").and_then(Value::as_str) == Some("user") {
                "user"
            } else {
                "assistant"
            };
            let timestamp_ms = entry
                .get("timestampMs")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(fallback_timestamp_ms);
            results.push(TranscriptSearchResult {
                agent_id: conversation_id.to_string(),
                entry_id: entry_id.to_string(),
                role: role.to_string(),
                timestamp_ms,
                snippet,
            });
            matched_count += 1;
        }
    }
    Ok(results)
}

fn human_entry_search_snippet(entry: &Value, normalized_query: &str) -> Option<String> {
    if entry.get("kind").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let content = entry
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(snippet) = build_content_snippet(content, normalized_query) {
        return Some(snippet);
    }
    let mut parts = Vec::new();
    if let Some(author_name) = entry
        .get("authorName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(author_name);
    }
    if let Some(origin_author) =
        ProductionSessionWorkers::human_entry_visible_forward_origin_author(entry)
    {
        if !parts.contains(&origin_author) {
            parts.push(origin_author);
        }
    }
    if !content.trim().is_empty() {
        parts.push(content.trim());
    }
    if let Some(attachments) = entry.get("attachments").and_then(Value::as_array) {
        for attachment in attachments {
            if let Some(name) = attachment
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                parts.push(name);
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    build_content_snippet(&parts.join(" "), normalized_query)
}

fn collect_human_conversation_media_results(
    session: &Arc<ProductionSessionWorkers>,
    query: &str,
    limit: usize,
) -> Result<Vec<RosterMediaSearchResult>, String> {
    if limit == 0 || session.local_human_id().is_err() {
        return Ok(Vec::new());
    }
    let normalized_query = query.trim().to_lowercase();
    let conversations = session.list_human_conversations()?;
    let mut results = Vec::new();
    for conversation in conversations {
        let Some(conversation_id) = conversation.get("id").and_then(Value::as_str) else {
            continue;
        };
        let fallback_timestamp_ms = conversation
            .get("updatedAt")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or_default();
        let Ok(entries) = session.read_human_conversation_transcript(conversation_id) else {
            continue;
        };
        for entry in entries.iter().rev() {
            if entry.get("kind").and_then(Value::as_str) != Some("message") {
                continue;
            }
            let Some(entry_id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(attachments) = entry.get("attachments").and_then(Value::as_array) else {
                continue;
            };
            let timestamp_ms = entry
                .get("timestampMs")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(fallback_timestamp_ms)
                .round() as i64;
            for attachment in attachments {
                let Some(file_name) = attachment
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let path = attachment
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ext = human_attachment_extension(file_name)
                    .or_else(|| human_attachment_extension(path))
                    .map(|extension| format!(".{extension}"))
                    .unwrap_or_default();
                let mime = attachment
                    .get("mimeType")
                    .or_else(|| attachment.get("contentType"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .or_else(|| human_attachment_mime(file_name));
                let kind = attachment_kind_name(classify_human_attachment(file_name, path)).to_string();
                let mut media_fields = vec![file_name, ext.as_str(), kind.as_str()];
                if let Some(mime_str) = mime.as_deref() {
                    media_fields.push(mime_str);
                }
                if let Some(author_name) = entry
                    .get("authorName")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    media_fields.push(author_name);
                }
                if let Some(origin_author) =
                    ProductionSessionWorkers::human_entry_visible_forward_origin_author(entry)
                {
                    media_fields.push(origin_author);
                }
                if let Some(caption) = entry
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    media_fields.push(caption);
                }
                let matches_query = normalized_query.is_empty()
                    || fabushi_messaging_core::query_matches_fields(
                        &normalized_query,
                        &media_fields,
                    );
                if !matches_query {
                    continue;
                }
                let width = attachment
                    .get("width")
                    .and_then(|value| {
                        value.as_i64().or_else(|| {
                            value
                                .as_f64()
                                .filter(|number| number.is_finite())
                                .map(|number| number.round() as i64)
                        })
                    })
                    .filter(|value| *value > 0);
                let height = attachment
                    .get("height")
                    .and_then(|value| {
                        value.as_i64().or_else(|| {
                            value
                                .as_f64()
                                .filter(|number| number.is_finite())
                                .map(|number| number.round() as i64)
                        })
                    })
                    .filter(|value| *value > 0);
                results.push(RosterMediaSearchResult {
                    agent_id: conversation_id.to_string(),
                    entry_id: entry_id.to_string(),
                    file_name: file_name.to_string(),
                    ext,
                    mime,
                    kind,
                    timestamp_ms,
                    width,
                    height,
                });
            }
        }
    }
    Ok(results)
}

fn human_attachment_extension(subject: &str) -> Option<String> {
    let base = subject.rsplit(['/', '\\']).next().unwrap_or("");
    let dot = base.rfind('.')?;
    if dot == 0 || dot + 1 >= base.len() {
        return None;
    }
    Some(base[dot + 1..].to_ascii_lowercase())
}

fn human_attachment_mime(file_name: &str) -> Option<String> {
    let extension = human_attachment_extension(file_name)?;
    let mime = match extension.as_str() {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "m4v" | "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "oga" | "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        _ => return None,
    };
    Some(mime.into())
}

fn classify_human_attachment(file_name: &str, url_or_path: &str) -> AttachmentKind {
    const IMAGES: &[&str] = &["avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "svg", "webp"];
    const VIDEOS: &[&str] = &["m4v", "mov", "mp4", "ogv", "webm"];
    const AUDIO: &[&str] = &["aac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "weba"];
    const MARKDOWN: &[&str] = &["md", "markdown", "mdx"];
    const TABLES: &[&str] = &["csv", "tsv", "xlsx", "xls"];
    const JSON: &[&str] = &["json", "jsonc", "json5", "ndjson"];
    const ARCHIVES: &[&str] = &[
        "zip", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "7z", "rar",
    ];
    const TEXT: &[&str] = &[
        "txt", "text", "log", "rst", "adoc", "tex", "xml", "yaml", "yml", "toml", "ini",
        "cfg", "conf", "env", "properties", "plist", "gradle", "html", "htm", "css", "scss",
        "sass", "less", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "py", "pyi",
        "rb", "go", "rs", "java", "kt", "kts", "c", "h", "cc", "cpp", "cxx", "hpp", "hh",
        "cs", "php", "swift", "scala", "dart", "lua", "pl", "pm", "r", "sql", "graphql",
        "gql", "proto", "vue", "svelte", "astro", "sh", "bash", "zsh", "fish", "bat", "ps1",
        "tf", "tfvars", "dockerfile", "diff", "patch",
    ];
    let classify = |subject: &str| -> Option<AttachmentKind> {
        let extension = human_attachment_extension(subject)?;
        let extension = extension.as_str();
        if IMAGES.contains(&extension) {
            Some(AttachmentKind::Image)
        } else if VIDEOS.contains(&extension) {
            Some(AttachmentKind::Video)
        } else if AUDIO.contains(&extension) {
            Some(AttachmentKind::Audio)
        } else if extension == "pdf" {
            Some(AttachmentKind::Pdf)
        } else if TABLES.contains(&extension) {
            Some(AttachmentKind::Table)
        } else if JSON.contains(&extension) {
            Some(AttachmentKind::Json)
        } else if MARKDOWN.contains(&extension) {
            Some(AttachmentKind::Markdown)
        } else if extension == "docx" {
            Some(AttachmentKind::Document)
        } else if TEXT.contains(&extension) {
            Some(AttachmentKind::Text)
        } else if ARCHIVES.contains(&extension) {
            Some(AttachmentKind::Archive)
        } else {
            None
        }
    };
    classify(file_name)
        .or_else(|| classify(url_or_path))
        .unwrap_or(AttachmentKind::File)
}

fn char_boundary_at_or_after(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn char_boundary_at_or_before(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
