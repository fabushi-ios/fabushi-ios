use std::fs::Metadata;
use std::io::{self, Seek, SeekFrom, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sha256::sha256_hex;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TranscriptOccurrenceConflictError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TranscriptJournalCorruptionError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TranscriptJournalWriteError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptCheckpoint {
    pub turns: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredTranscriptStep {
    pub turn_index: usize,
    pub step_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTranscriptCursor {
    pub turn_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_step: Option<DeferredTranscriptStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTranscriptCheckpoint {
    pub version: u8,
    pub previous_checkpoint_hash: String,
    pub checkpoint_hash: String,
    pub append_offset: u64,
    pub file_device: String,
    pub file_inode: String,
    pub lines: Vec<String>,
    pub cursor: PendingTranscriptCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub size: u64,
    pub device: String,
    pub inode: String,
}

pub fn sha256(value: impl AsRef<[u8]>) -> String {
    sha256_hex(value)
}

pub fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    left == right
}

pub fn checkpoint_identity(checkpoint: &TranscriptCheckpoint) -> String {
    let len = checkpoint.turns.len();
    let prior = len
        .checked_sub(2)
        .and_then(|index| checkpoint.turns.get(index))
        .map(|value| hex(value))
        .unwrap_or_default();
    let last = len
        .checked_sub(1)
        .and_then(|index| checkpoint.turns.get(index))
        .map(|value| hex(value))
        .unwrap_or_default();
    sha256(format!("{len}:{prior}:{last}"))
}

pub fn is_missing_file(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

pub fn file_identity(metadata: &Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return FileIdentity {
            size: metadata.size(),
            device: metadata.dev().to_string(),
            inode: metadata.ino().to_string(),
        };
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            size: metadata.len(),
            device: "0".to_string(),
            inode: "0".to_string(),
        }
    }
}

fn safe_non_negative_integer(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return (value <= MAX_SAFE_INTEGER).then_some(value);
    }
    let value = value.as_f64()?;
    (value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value <= MAX_SAFE_INTEGER as f64)
        .then_some(value as u64)
}

pub fn parse_deferred_step(
    value: Option<&Value>,
) -> Result<Option<DeferredTranscriptStep>, TranscriptJournalCorruptionError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value.as_object().ok_or_else(|| {
        TranscriptJournalCorruptionError("transcript deferred cursor is invalid".into())
    })?;
    let turn_index = object
        .get("turnIndex")
        .and_then(safe_non_negative_integer)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            TranscriptJournalCorruptionError("transcript deferred cursor is invalid".into())
        })?;
    let step_index = object
        .get("stepIndex")
        .and_then(safe_non_negative_integer)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            TranscriptJournalCorruptionError("transcript deferred cursor is invalid".into())
        })?;
    Ok(Some(DeferredTranscriptStep {
        turn_index,
        step_index,
    }))
}

fn valid_hash(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    (value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
        .then(|| value.to_string())
}

pub fn parse_pending_checkpoint(
    raw: &str,
) -> Result<PendingTranscriptCheckpoint, TranscriptJournalCorruptionError> {
    let parsed: Value = serde_json::from_str(raw).map_err(|error| {
        TranscriptJournalCorruptionError(format!(
            "pending transcript checkpoint is not valid JSON: {error}"
        ))
    })?;
    let object = parsed.as_object().ok_or_else(|| {
        TranscriptJournalCorruptionError(
            "pending transcript checkpoint is not an object".into(),
        )
    })?;
    let cursor = object
        .get("cursor")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .filter(|value| *value == 1)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })? as u8;
    let previous_checkpoint_hash = object
        .get("previousCheckpointHash")
        .and_then(valid_hash)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let checkpoint_hash = object
        .get("checkpointHash")
        .and_then(valid_hash)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let append_offset = object
        .get("appendOffset")
        .and_then(safe_non_negative_integer)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let file_device = object
        .get("fileDevice")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let file_inode = object
        .get("fileInode")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let raw_lines = object.get("lines").and_then(Value::as_array).ok_or_else(|| {
        TranscriptJournalCorruptionError(
            "pending transcript checkpoint metadata is invalid".into(),
        )
    })?;
    let mut lines = Vec::with_capacity(raw_lines.len());
    for line in raw_lines {
        let line = line.as_str().ok_or_else(|| {
            TranscriptJournalCorruptionError("pending transcript line is invalid".into())
        })?;
        let parsed: Value = serde_json::from_str(line).map_err(|_| {
            TranscriptJournalCorruptionError("pending transcript line is invalid".into())
        })?;
        let envelope = parsed.as_object().ok_or_else(|| {
            TranscriptJournalCorruptionError("pending transcript line is invalid".into())
        })?;
        if !envelope.contains_key("role") || !envelope.contains_key("message") {
            return Err(TranscriptJournalCorruptionError(
                "pending transcript line does not use the legacy message envelope".into(),
            ));
        }
        lines.push(line.to_string());
    }
    let turn_count = cursor
        .get("turnCount")
        .and_then(safe_non_negative_integer)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint cursor is invalid".into(),
            )
        })?;
    let deferred_step = parse_deferred_step(cursor.get("deferredStep"))?;

    Ok(PendingTranscriptCheckpoint {
        version,
        previous_checkpoint_hash,
        checkpoint_hash,
        append_offset,
        file_device,
        file_inode,
        lines,
        cursor: PendingTranscriptCursor {
            turn_count,
            deferred_step,
        },
    })
}

pub fn write_all_at<W: Write + Seek>(
    handle: &mut W,
    bytes: &[u8],
    position: u64,
) -> Result<u64, TranscriptJournalWriteError> {
    handle
        .seek(SeekFrom::Start(position))
        .map_err(|error| TranscriptJournalWriteError(error.to_string()))?;
    let mut written = 0usize;
    while written < bytes.len() {
        let count = handle
            .write(&bytes[written..])
            .map_err(|error| TranscriptJournalWriteError(error.to_string()))?;
        if count == 0 {
            return Err(TranscriptJournalWriteError(
                "transcript JSONL write made no progress".into(),
            ));
        }
        written = written.saturating_add(count);
    }
    Ok(position.saturating_add(written as u64))
}

pub fn format_text_line(role: &str, text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(
        serde_json::json!({
            "role": role,
            "message": {
                "content": [{
                    "type": "text",
                    "text": text,
                }]
            }
        })
        .to_string(),
    )
}

pub fn format_tool_line(role: &str, name: &str, payload: Value) -> String {
    let content = if role == "assistant" {
        serde_json::json!([{
            "type": "tool_use",
            "name": name,
            "input": payload,
        }])
    } else {
        serde_json::json!([{
            "type": "tool_result",
            "name": name,
            "result": payload,
        }])
    };
    serde_json::json!({
        "role": role,
        "message": {
            "content": content,
        }
    })
    .to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
