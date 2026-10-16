use crate::conversation_state_binary::{
    TranscriptMirrorConversationState, decode_summary_archive_message_ids,
};
use crate::transcript_occurrence_deriver::{
    TranscriptOccurrenceBlobStore, strip_context_tags, strip_hidden_thinking_tags,
};
use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const OVERSIZE_TRANSCRIPT_BLOB_THRESHOLD_BYTES: usize = 5_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyTranscriptMirrorError(pub String);

impl std::fmt::Display for LegacyTranscriptMirrorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LegacyTranscriptMirrorError {}

fn error(message: impl Into<String>) -> LegacyTranscriptMirrorError {
    LegacyTranscriptMirrorError(message.into())
}

fn binary_marker_placeholder(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                binary_marker_placeholder(value);
            }
        }
        Value::Object(object) => {
            let marker = object
                .get("__type")
                .and_then(Value::as_str)
                .is_some_and(|marker| marker == "Uint8Array");
            let hex = object.get("hex").and_then(Value::as_str);
            if marker {
                if let Some(hex) = hex {
                    *value = Value::String(format!(
                        "[Binary data omitted from transcript: {} bytes]",
                        hex.len() / 2
                    ));
                    return;
                }
            }
            for value in object.values_mut() {
                binary_marker_placeholder(value);
            }
        }
        _ => {}
    }
}

fn deserialize_core_message(blob: &[u8]) -> Option<Value> {
    let mut value = serde_json::from_slice::<Value>(blob).ok()?;
    binary_marker_placeholder(&mut value);
    value.as_object()?;
    Some(value)
}

fn role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

fn is_summary_message(message: &Value) -> bool {
    message
        .pointer("/providerOptions/cursor/isSummary")
        .and_then(Value::as_bool)
        == Some(true)
}

fn create_oversize_blob_omitted_message(blob_size_bytes: usize) -> Value {
    json!({
        "role":"assistant",
        "content":format!(
            "[Oversize transcript blob omitted: {:.1} MB]",
            blob_size_bytes as f64 / 1_000_000.0
        )
    })
}

fn hydrate_blob_ids<Store: TranscriptOccurrenceBlobStore>(
    store: &Store,
    blob_ids: &[Vec<u8>],
) -> Vec<Value> {
    let mut messages = Vec::new();
    for blob_id in blob_ids {
        let Some(blob) = store.get_blob(blob_id) else {
            continue;
        };
        if blob.len() > OVERSIZE_TRANSCRIPT_BLOB_THRESHOLD_BYTES {
            messages.push(create_oversize_blob_omitted_message(blob.len()));
            continue;
        }
        if let Some(message) = deserialize_core_message(&blob) {
            messages.push(message);
        }
    }
    messages
}

fn hydrate_messages<Store: TranscriptOccurrenceBlobStore>(
    store: &Store,
    state: &TranscriptMirrorConversationState,
) -> Vec<Value> {
    let mut messages = Vec::new();
    for archive_ref in &state.summary_archives {
        let Some(archive_blob) = store.get_blob(archive_ref) else {
            continue;
        };
        let Ok(message_ids) = decode_summary_archive_message_ids(&archive_blob) else {
            continue;
        };
        messages.extend(hydrate_blob_ids(store, &message_ids));
    }

    messages.extend(
        hydrate_blob_ids(store, &state.root_prompt_messages_json)
            .into_iter()
            .filter(|message| role(message) != Some("system") && !is_summary_message(message)),
    );
    messages
}

fn visible_string_content(role: &str, content: &str) -> String {
    let content = if role == "user" {
        strip_context_tags(content)
    } else {
        content.to_owned()
    };
    if role == "assistant" {
        strip_hidden_thinking_tags(&content)
    } else {
        content
    }
}

fn tool_use_part(part: &Map<String, Value>) -> Value {
    let mut value = Map::new();
    value.insert("type".into(), Value::String("tool_use".into()));
    if let Some(name) = part.get("toolName").cloned() {
        value.insert("name".into(), name);
    }
    value.insert(
        "input".into(),
        part.get("args").cloned().unwrap_or(Value::Null),
    );
    Value::Object(value)
}

fn format_single_message_jsonl(message: &Value) -> Option<String> {
    let role = role(message)?;
    if role == "system" || is_summary_message(message) {
        return None;
    }

    let mut text_parts = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut tool_calls = Vec::new();

    match message.get("content") {
        Some(Value::String(content)) => {
            let visible = visible_string_content(role, content);
            if !visible.trim().is_empty() {
                text_parts.push(visible);
            }
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let Some(text) = part.get("text").and_then(Value::as_str) else {
                            continue;
                        };
                        let visible = visible_string_content(role, text);
                        if !visible.trim().is_empty() {
                            text_parts.push(visible);
                        }
                    }
                    Some("reasoning") => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                thinking_parts.push(text.to_owned());
                            }
                        }
                    }
                    Some("redacted-reasoning") => thinking_parts.push("[REDACTED]".into()),
                    Some("image") => text_parts.push("[Image]".into()),
                    Some("file") => {
                        text_parts.push(match part.get("filename").and_then(Value::as_str) {
                            Some(filename) => format!("[File: {filename}]"),
                            None => "[File]".into(),
                        });
                    }
                    Some("tool-call") => tool_calls.push(tool_use_part(part)),
                    Some("tool-result") | _ => {}
                }
            }
        }
        _ => {}
    }

    let mut content = Vec::new();
    let mut visible_text = Vec::new();
    if !text_parts.is_empty() {
        visible_text.push(text_parts.join("\n"));
    }
    if !thinking_parts.is_empty() {
        visible_text.push(thinking_parts.join("\n"));
    }
    if !visible_text.is_empty() {
        content.push(json!({
            "type":"text",
            "text":visible_text.join("\n\n")
        }));
    }
    content.extend(tool_calls);
    if content.is_empty() {
        return None;
    }
    Some(json!({"role":role,"message":{"content":content}}).to_string())
}

fn format_transcript_jsonl(messages: &[Value]) -> String {
    let mut filtered = messages
        .iter()
        .filter(|message| role(message) != Some("system"))
        .collect::<Vec<_>>();
    if filtered.len() >= 2
        && role(filtered[0]) == Some("user")
        && role(filtered[1]) == Some("user")
    {
        filtered.remove(0);
    }
    filtered
        .into_iter()
        .filter_map(format_single_message_jsonl)
        .collect::<Vec<_>>()
        .join("\n")
}

fn ensure_trailing_newline(mut content: String) -> String {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

fn count_transcript_message_lines(content: &str) -> usize {
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| {
            value.get("type").is_none()
                && value.get("role").is_some()
                && value.get("message").is_some()
        })
        .count()
}

pub struct LegacyFileTranscriptMirror {
    transcripts_dir: PathBuf,
}

impl LegacyFileTranscriptMirror {
    pub fn new(transcripts_dir: impl Into<PathBuf>) -> Self {
        Self {
            transcripts_dir: transcripts_dir.into(),
        }
    }

    pub fn jsonl_path_for(
        &self,
        conversation_id: &str,
    ) -> Result<PathBuf, LegacyTranscriptMirrorError> {
        let safe = crate::package_utils_workspace_paths::get_safe_conversation_id(conversation_id);
        Ok(self
            .transcripts_dir
            .join(&safe)
            .join(format!("{safe}.jsonl")))
    }

    fn write_file(
        &self,
        path: &Path,
        content: &str,
    ) -> Result<(), LegacyTranscriptMirrorError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|io_error| error(format!("create transcript directory: {io_error}")))?;
        }
        fs::write(path, content)
            .map_err(|io_error| error(format!("write transcript file: {io_error}")))
    }

    pub fn write_full<Store: TranscriptOccurrenceBlobStore>(
        &self,
        conversation_id: &str,
        state: &TranscriptMirrorConversationState,
        store: &Store,
    ) -> Result<bool, LegacyTranscriptMirrorError> {
        let messages = hydrate_messages(store, state);
        let formatted = format_transcript_jsonl(&messages);
        if formatted.is_empty() {
            return Ok(
                state.summary_archives.is_empty() && state.root_prompt_messages_json.is_empty()
            );
        }

        let path = self.jsonl_path_for(conversation_id)?;
        if let Ok(existing) = fs::read_to_string(&path) {
            let existing_count = count_transcript_message_lines(&existing);
            let new_count = count_transcript_message_lines(&formatted);
            if new_count < existing_count {
                return Ok(false);
            }
        }

        self.write_file(&path, &ensure_trailing_newline(formatted))?;
        Ok(true)
    }

    pub fn write_incremental<Store: TranscriptOccurrenceBlobStore>(
        &self,
        conversation_id: &str,
        state: &TranscriptMirrorConversationState,
        store: &Store,
        previous_root_prompt_count: usize,
    ) -> Result<Option<usize>, LegacyTranscriptMirrorError> {
        let current_count = state.root_prompt_messages_json.len();
        if previous_root_prompt_count == 0 || current_count < previous_root_prompt_count {
            return Ok(None);
        }
        if current_count == previous_root_prompt_count {
            return Ok(Some(current_count));
        }

        let messages = hydrate_blob_ids(
            store,
            &state.root_prompt_messages_json[previous_root_prompt_count..],
        );
        let formatted = format_transcript_jsonl(&messages);
        if !formatted.is_empty() {
            let path = self.jsonl_path_for(conversation_id)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|io_error| error(format!("create transcript directory: {io_error}")))?;
            }
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| file.write_all(ensure_trailing_newline(formatted).as_bytes()))
                .map_err(|io_error| error(format!("append transcript file: {io_error}")))?;
        }
        Ok(Some(current_count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct Store(HashMap<Vec<u8>, Vec<u8>>);

    impl Store {
        fn insert(&mut self, id: u8, body: impl Into<Vec<u8>>) {
            self.0.insert(vec![id], body.into());
        }
    }

    impl TranscriptOccurrenceBlobStore for Store {
        fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
            self.0.get(id).cloned()
        }
    }

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-legacy-transcript-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn bytes_field(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut tag = field * 8 + 2;
        loop {
            let mut byte = (tag & 0x7f) as u8;
            tag >>= 7;
            if tag != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if tag == 0 {
                break;
            }
        }
        let mut length = value.len() as u64;
        loop {
            let mut byte = (length & 0x7f) as u8;
            length >>= 7;
            if length != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if length == 0 {
                break;
            }
        }
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn full_write_hydrates_archives_strips_context_and_binary_markers() {
        let root = temp_dir();
        let mut store = Store::default();
        store.insert(
            1,
            br#"{"role":"user","content":"first bootstrap"}"#.to_vec(),
        );
        store.insert(
            2,
            br#"{"role":"user","content":"<user_info>secret</user_info>Hello"}"#.to_vec(),
        );
        store.insert(
            3,
            br#"{"role":"assistant","content":[{"type":"text","text":"<think>private</think>Visible"},{"type":"tool-call","toolName":"search","args":{"blob":{"__type":"Uint8Array","hex":"0102"}}}]}"#.to_vec(),
        );
        store.insert(
            4,
            br#"{"role":"assistant","content":"Archived"}"#.to_vec(),
        );
        store.insert(9, bytes_field(1, &[4]));

        let state = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1], vec![2], vec![3]],
            turns: vec![],
            summary_archives: vec![vec![9]],
        };
        let mirror = LegacyFileTranscriptMirror::new(&root);
        assert!(mirror.write_full("conversation-a", &state, &store).unwrap());

        let content = fs::read_to_string(mirror.jsonl_path_for("conversation-a").unwrap()).unwrap();
        assert!(content.contains("Archived"));
        assert!(content.contains("Hello"));
        assert!(!content.contains("secret"));
        assert!(content.contains("Visible"));
        assert!(!content.contains("private"));
        assert!(content.contains("Binary data omitted from transcript: 2 bytes"));
        // Pinned Grok 0.18 only drops the leading bootstrap when the hydrated
        // transcript itself starts with two user messages. Summary archives are
        // hydrated first, so an archived assistant message keeps this root prompt.
        assert!(content.contains("first bootstrap"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn full_write_refuses_to_shrink_existing_transcript() {
        let root = temp_dir();
        let mut store = Store::default();
        store.insert(1, br#"{"role":"user","content":"one"}"#.to_vec());
        store.insert(2, br#"{"role":"assistant","content":"two"}"#.to_vec());
        let mirror = LegacyFileTranscriptMirror::new(&root);
        let path = mirror.jsonl_path_for("conversation-a").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"a\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"b\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"c\"}]}}\n"
            ),
        )
        .unwrap();

        let state = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1], vec![2]],
            turns: vec![],
            summary_archives: vec![],
        };
        assert!(!mirror.write_full("conversation-a", &state, &store).unwrap());
        assert_eq!(count_transcript_message_lines(&fs::read_to_string(path).unwrap()), 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incremental_appends_only_new_root_prompt_messages() {
        let root = temp_dir();
        let mut store = Store::default();
        store.insert(1, br#"{"role":"user","content":"one"}"#.to_vec());
        store.insert(2, br#"{"role":"assistant","content":"two"}"#.to_vec());
        let mirror = LegacyFileTranscriptMirror::new(&root);
        let first = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1]],
            turns: vec![],
            summary_archives: vec![],
        };
        assert!(mirror.write_full("conversation-a", &first, &store).unwrap());
        let second = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1], vec![2]],
            turns: vec![],
            summary_archives: vec![],
        };
        assert_eq!(
            mirror
                .write_incremental("conversation-a", &second, &store, 1)
                .unwrap(),
            Some(2)
        );
        let content = fs::read_to_string(mirror.jsonl_path_for("conversation-a").unwrap()).unwrap();
        assert!(content.contains("one"));
        assert!(content.contains("two"));
        let _ = fs::remove_dir_all(root);
    }
}
