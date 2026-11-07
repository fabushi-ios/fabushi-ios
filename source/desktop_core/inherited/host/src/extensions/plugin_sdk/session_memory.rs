use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const DEFAULT_PAGE_LIMIT: usize = 20;
pub const MAX_PAGE_LIMIT: usize = 100;
pub const MAX_CURSOR_LENGTH: usize = 128;
pub const MAX_TRANSCRIPT_ITEM_BYTES: usize = 512 * 1024; // 512 KB
pub const MAX_TRANSCRIPT_PAGE_BYTES: usize = 20 * 1024 * 1024; // 20 MB
pub const VISIBLE_TRANSCRIPT_CURSOR_VERSION: u8 = 1;
pub const MAX_VISIBLE_TRANSCRIPT_CURSOR_LENGTH: usize = 4096;
pub const DEFAULT_VISIBLE_TRANSCRIPT_MAX_MESSAGES: usize = 1_000;
pub const MAX_VISIBLE_TRANSCRIPT_MAX_MESSAGES: usize = 10_000;
pub const DEFAULT_VISIBLE_TRANSCRIPT_MAX_BYTES: usize = 1_000_000;
pub const MAX_VISIBLE_TRANSCRIPT_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionVisibility {
    Public,
    Direct,
    Incognito,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub thread_id: String,
    pub title: String,
    pub visibility: SessionVisibility,
    pub updated_at_ms: u64,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCatalogQueryParams {
    pub search_term: Option<String>,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for SessionCatalogQueryParams {
    fn default() -> Self {
        Self {
            search_term: None,
            limit: DEFAULT_PAGE_LIMIT,
            cursor: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCatalogPage {
    pub items: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
    pub total_count: usize,
}

pub fn normalize_page_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        limit.min(MAX_PAGE_LIMIT)
    }
}

pub fn paginate_sessions(
    all_sessions: &[SessionSummary],
    params: &SessionCatalogQueryParams,
) -> Result<SessionCatalogPage, String> {
    if let Some(cursor) = &params.cursor {
        if cursor.len() > MAX_CURSOR_LENGTH {
            return Err(format!(
                "Cursor length {} exceeds maximum allowed {}",
                cursor.len(),
                MAX_CURSOR_LENGTH
            ));
        }
    }

    let limit = normalize_page_limit(params.limit);

    let filtered: Vec<&SessionSummary> = all_sessions
        .iter()
        .filter(|s| {
            if let Some(ref term) = params.search_term {
                let term_lower = term.to_ascii_lowercase();
                s.title.to_ascii_lowercase().contains(&term_lower)
                    || s.session_id.to_ascii_lowercase().contains(&term_lower)
                    || s.thread_id.to_ascii_lowercase().contains(&term_lower)
            } else {
                true
            }
        })
        .collect();

    let total_count = filtered.len();

    // Cursor is 0-based offset encoded as string
    let start_idx = if let Some(cursor_str) = &params.cursor {
        cursor_str
            .parse::<usize>()
            .map_err(|_| "Invalid cursor offset format".to_string())?
    } else {
        0
    };

    if start_idx >= total_count {
        return Ok(SessionCatalogPage {
            items: Vec::new(),
            next_cursor: None,
            total_count,
        });
    }

    let end_idx = (start_idx + limit).min(total_count);
    let items = filtered[start_idx..end_idx]
        .iter()
        .map(|&s| s.clone())
        .collect();

    let next_cursor = if end_idx < total_count {
        Some(end_idx.to_string())
    } else {
        None
    };

    Ok(SessionCatalogPage {
        items,
        next_cursor,
        total_count,
    })
}

/// Select the visible portion of a transcript after the canonical session
/// owner has resolved a reset-window boundary.
///
/// This is deliberately a pure projection. `reset_tail_start` is supplied by
/// the existing transcript/session owner; this module does not infer, store,
/// or mutate reset generations. Tool results are visible only when the same
/// window contains a corresponding assistant tool call, matching the SDK
/// context-engine contract and avoiding orphan result injection.
pub fn select_visible_transcript_messages(
    entries: &[Value],
    reset_tail_start: Option<usize>,
) -> Vec<Value> {
    let start = reset_tail_start.unwrap_or(0).min(entries.len());
    let window = &entries[start..];
    let paired_tool_call_ids = window
        .iter()
        .flat_map(tool_call_ids)
        .collect::<std::collections::HashSet<_>>();

    window
        .iter()
        .filter(|entry| {
            let Some(message) = message_object(entry) else {
                return true;
            };
            if message.get("role").and_then(Value::as_str) != Some("toolResult") {
                return true;
            }
            message
                .get("toolCallId")
                .and_then(Value::as_str)
                .is_some_and(|tool_call_id| paired_tool_call_ids.contains(tool_call_id))
        })
        .cloned()
        .collect()
}

fn message_object(entry: &Value) -> Option<&Map<String, Value>> {
    entry
        .as_object()
        .and_then(|object| object.get("message").and_then(Value::as_object).or(Some(object)))
}

fn tool_call_ids(entry: &Value) -> Vec<&str> {
    let Some(message) = message_object(entry) else {
        return Vec::new();
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("toolCall"))
        .filter_map(|part| part.get("id").and_then(Value::as_str))
        .collect()
}

/// Opaque, generation-aware continuation state for visible transcript reads.
/// The transcript/session owner supplies the generation and reset boundary;
/// this type only validates and transports the continuation hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleTranscriptCursor {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub generation: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "lastEventSeq")]
    pub last_event_seq: i64,
    #[serde(rename = "lastMessagePosition")]
    pub last_message_position: i64,
    #[serde(rename = "resetBoundarySeq", skip_serializing_if = "Option::is_none")]
    pub reset_boundary_seq: Option<i64>,
    #[serde(rename = "resetBoundaryId", skip_serializing_if = "Option::is_none")]
    pub reset_boundary_id: Option<String>,
    pub version: u8,
}

pub fn new_visible_transcript_cursor(
    agent_id: impl Into<String>,
    session_id: impl Into<String>,
    generation: impl Into<String>,
) -> VisibleTranscriptCursor {
    VisibleTranscriptCursor {
        agent_id: agent_id.into(),
        generation: generation.into(),
        session_id: session_id.into(),
        last_event_seq: -1,
        last_message_position: -1,
        reset_boundary_seq: None,
        reset_boundary_id: None,
        version: VISIBLE_TRANSCRIPT_CURSOR_VERSION,
    }
}

pub fn encode_visible_transcript_cursor(
    cursor: &VisibleTranscriptCursor,
) -> Result<String, String> {
    validate_visible_transcript_cursor(cursor)?;
    let bytes = serde_json::to_vec(cursor).map_err(|error| format!("cursor encode failed: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_visible_transcript_cursor(value: &str) -> Option<VisibleTranscriptCursor> {
    if value.is_empty() || value.len() > MAX_VISIBLE_TRANSCRIPT_CURSOR_LENGTH {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    let cursor = serde_json::from_slice::<VisibleTranscriptCursor>(&bytes).ok()?;
    validate_visible_transcript_cursor(&cursor).ok()?;
    (encode_visible_transcript_cursor(&cursor).ok()?.as_str() == value).then_some(cursor)
}

fn validate_visible_transcript_cursor(cursor: &VisibleTranscriptCursor) -> Result<(), String> {
    if cursor.version != VISIBLE_TRANSCRIPT_CURSOR_VERSION
        || cursor.agent_id.is_empty()
        || cursor.session_id.is_empty()
        || cursor.generation.is_empty()
        || cursor.last_event_seq < -1
        || cursor.last_message_position < -1
        || (cursor.last_event_seq == -1) != (cursor.last_message_position == -1)
        || cursor
            .reset_boundary_seq
            .is_some_and(|sequence| sequence < -1)
        || cursor
            .reset_boundary_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || cursor.reset_boundary_seq.unwrap_or(-1) < 0)
    {
        return Err("invalid visible transcript cursor".into());
    }
    Ok(())
}

pub fn with_visible_reset_boundary(
    mut cursor: VisibleTranscriptCursor,
    latest: Option<(i64, Option<&str>)>,
) -> Result<VisibleTranscriptCursor, String> {
    let (sequence, event_id) = latest.map_or((-1, None), |(sequence, event_id)| {
        (sequence, event_id)
    });
    if sequence < -1 {
        return Err("invalid visible transcript reset boundary".into());
    }
    cursor.reset_boundary_seq = Some(sequence);
    cursor.reset_boundary_id = event_id.map(str::to_owned);
    validate_visible_transcript_cursor(&cursor)?;
    Ok(cursor)
}

pub fn visible_reset_boundary_changed(
    cursor: &VisibleTranscriptCursor,
    latest: Option<(i64, Option<&str>)>,
    same_generation: bool,
) -> bool {
    if same_generation {
        latest.map_or(-1, |(sequence, _)| sequence) != cursor.reset_boundary_seq.unwrap_or(-1)
    } else {
        latest.and_then(|(_, event_id)| event_id) != cursor.reset_boundary_id.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleTranscriptStart {
    Transcript,
    ResetWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleTranscriptResetReason {
    AnchorMissing,
    AnchorMoved,
    GenerationMismatch,
    InvalidCursor,
    ScopeMismatch,
    SessionReset,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VisibleTranscriptReadResult {
    Page {
        cursor: String,
        entries: Vec<Value>,
        has_more: bool,
        serialized_bytes: usize,
        required_bytes: Option<usize>,
    },
    Reset {
        cursor: String,
        reason: VisibleTranscriptResetReason,
    },
}

/// A read-only fence captured by the existing user-turn admission owner.
/// `before_message_position` is exclusive in the already projected visible
/// snapshot; the fence carries identity so a caller cannot apply it to a
/// different transcript or generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTranscriptReadFence {
    pub agent_id: String,
    pub session_id: String,
    pub generation: String,
    pub before_message_position: usize,
}

/// Read one bounded visible page from a caller-owned snapshot.
///
/// The snapshot, generation and reset boundary are supplied by the existing
/// transcript owner. This function only applies cursor validation, reset-window
/// selection, paired-result filtering, message/byte budgets, and continuation.
pub fn read_visible_transcript_page(
    entries: &[Value],
    agent_id: &str,
    session_id: &str,
    generation: &str,
    latest_reset: Option<(i64, Option<&str>)>,
    reset_tail_start: Option<usize>,
    start: VisibleTranscriptStart,
    cursor: Option<&str>,
    read_fence: Option<&VisibleTranscriptReadFence>,
    max_messages: usize,
    max_bytes: usize,
) -> Result<VisibleTranscriptReadResult, String> {
    if max_messages == 0 || max_messages > MAX_VISIBLE_TRANSCRIPT_MAX_MESSAGES {
        return Err(format!(
            "max_messages must be between 1 and {}",
            MAX_VISIBLE_TRANSCRIPT_MAX_MESSAGES
        ));
    }
    if max_bytes == 0 || max_bytes > MAX_VISIBLE_TRANSCRIPT_MAX_BYTES {
        return Err(format!(
            "max_bytes must be between 1 and {}",
            MAX_VISIBLE_TRANSCRIPT_MAX_BYTES
        ));
    }
    if let Some(fence) = read_fence {
        if fence.agent_id != agent_id || fence.session_id != session_id {
            return Err("current-turn transcript fence belongs to another scope".into());
        }
        if fence.generation != generation {
            return Err("current-turn transcript fence belongs to another generation".into());
        }
    }

    let reset_window = matches!(start, VisibleTranscriptStart::ResetWindow);
    let fresh_cursor = || {
        let cursor = new_visible_transcript_cursor(agent_id, session_id, generation);
        if reset_window {
            with_visible_reset_boundary(cursor, latest_reset).unwrap_or_else(|_| {
                new_visible_transcript_cursor(agent_id, session_id, generation)
            })
        } else {
            cursor
        }
    };

    let mut continuation = match cursor {
        None => fresh_cursor(),
        Some(value) => match decode_visible_transcript_cursor(value) {
            Some(cursor) => cursor,
            None => {
                let cursor = encode_visible_transcript_cursor(&fresh_cursor())?;
                return Ok(VisibleTranscriptReadResult::Reset {
                    cursor,
                    reason: VisibleTranscriptResetReason::InvalidCursor,
                });
            }
        },
    };

    if continuation.agent_id != agent_id || continuation.session_id != session_id {
        let cursor = encode_visible_transcript_cursor(&fresh_cursor())?;
        return Ok(VisibleTranscriptReadResult::Reset {
            cursor,
            reason: VisibleTranscriptResetReason::ScopeMismatch,
        });
    }
    if continuation.generation != generation {
        let cursor = encode_visible_transcript_cursor(&fresh_cursor())?;
        return Ok(VisibleTranscriptReadResult::Reset {
            cursor,
            reason: VisibleTranscriptResetReason::GenerationMismatch,
        });
    }
    if continuation.reset_boundary_seq.is_some()
        && visible_reset_boundary_changed(&continuation, latest_reset, true)
    {
        let cursor = encode_visible_transcript_cursor(&fresh_cursor())?;
        return Ok(VisibleTranscriptReadResult::Reset {
            cursor,
            reason: VisibleTranscriptResetReason::SessionReset,
        });
    }

    let start_index = if reset_window {
        reset_tail_start.unwrap_or(0).min(entries.len())
    } else {
        0
    };
    let visible = select_visible_transcript_messages(entries, Some(start_index));
    let visible_len = read_fence
        .map(|fence| fence.before_message_position.min(visible.len()))
        .unwrap_or(visible.len());
    let next_index = continuation.last_message_position.saturating_add(1) as usize;
    if next_index >= visible_len {
        return Ok(VisibleTranscriptReadResult::Page {
            cursor: encode_visible_transcript_cursor(&continuation)?,
            entries: Vec::new(),
            has_more: false,
            serialized_bytes: 0,
            required_bytes: None,
        });
    }

    let mut selected = Vec::new();
    let mut serialized_bytes = 0;
    let mut required_bytes = None;
    for entry in visible.iter().take(visible_len).skip(next_index) {
        let entry_bytes = serde_json::to_vec(entry)
            .map_err(|error| format!("transcript entry encode failed: {error}"))?;
        if selected.len() >= max_messages || serialized_bytes + entry_bytes.len() > max_bytes {
            if selected.is_empty() {
                required_bytes = Some(entry_bytes.len());
            }
            break;
        }
        serialized_bytes += entry_bytes.len();
        selected.push(entry.clone());
    }
    if selected.is_empty() {
        return Ok(VisibleTranscriptReadResult::Page {
            cursor: encode_visible_transcript_cursor(&continuation)?,
            entries: selected,
            has_more: true,
            serialized_bytes,
            required_bytes,
        });
    }
    let last_position = next_index + selected.len() - 1;
    continuation.last_message_position = last_position as i64;
    continuation.last_event_seq = last_position as i64;
    let has_more = last_position + 1 < visible_len;
    Ok(VisibleTranscriptReadResult::Page {
        cursor: encode_visible_transcript_cursor(&continuation)?,
        entries: selected,
        has_more,
        serialized_bytes,
        required_bytes,
    })
}

#[derive(Clone, Default)]
pub struct SessionTranscriptLockRegistry {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl SessionTranscriptLockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn acquire_lock(&self, agent_id: &str, session_id: &str) -> Arc<Mutex<()>> {
        let key = format!("{agent_id}:{session_id}");
        let mut locks = self.locks.lock().unwrap();
        locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    }

    pub fn enforce_sync_allowed(&self, visibility: SessionVisibility) -> Result<(), String> {
        if visibility == SessionVisibility::Incognito {
            Err("IncognitoSessionSyncAccessDenied: session is ephemeral and persistence sync is forbidden".to_string())
        } else {
            Ok(())
        }
    }
}
