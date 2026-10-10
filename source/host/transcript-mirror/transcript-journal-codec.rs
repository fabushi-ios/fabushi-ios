use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::Metadata;
use std::io::{Seek, SeekFrom, Write};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptOccurrenceConflictError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptJournalCorruptionError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptJournalWriteError(pub String);

macro_rules! impl_error {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
        impl std::error::Error for $name {}
    };
}
impl_error!(TranscriptOccurrenceConflictError);
impl_error!(TranscriptJournalCorruptionError);
impl_error!(TranscriptJournalWriteError);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptCheckpoint {
    pub turns: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeferredTranscriptStep {
    pub turn_index: usize,
    pub step_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTranscriptCheckpoint {
    pub version: u8,
    pub previous_checkpoint_hash: String,
    pub checkpoint_hash: String,
    pub append_offset: u64,
    pub file_device: String,
    pub file_inode: String,
    pub lines: Vec<String>,
    pub turn_count: usize,
    pub deferred_step: Option<DeferredTranscriptStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub size: u64,
    pub device: String,
    pub inode: String,
}

pub fn sha256(value: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(value.as_ref());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    left == right
}

pub fn checkpoint_identity(checkpoint: &TranscriptCheckpoint) -> String {
    let len = checkpoint.turns.len();
    let second_last = if len >= 2 {
        hex(&checkpoint.turns[len - 2])
    } else {
        String::new()
    };
    let last = checkpoint.turns.last().map(|turn| hex(turn)).unwrap_or_default();
    sha256(format!("{len}:{second_last}:{last}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(unix)]
pub fn file_identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        size: metadata.len(),
        device: metadata.dev().to_string(),
        inode: metadata.ino().to_string(),
    }
}

#[cfg(not(unix))]
pub fn file_identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        size: metadata.len(),
        device: "0".to_owned(),
        inode: "0".to_owned(),
    }
}

pub fn parse_deferred_step(
    value: &Value,
) -> Result<Option<DeferredTranscriptStep>, TranscriptJournalCorruptionError> {
    if value.is_null() {
        return Ok(None);
    }
    let Some(record) = value.as_object() else {
        return Err(TranscriptJournalCorruptionError(
            "transcript deferred cursor is invalid".into(),
        ));
    };
    let turn_index = nonnegative_usize(record.get("turnIndex")).ok_or_else(|| {
        TranscriptJournalCorruptionError("transcript deferred cursor is invalid".into())
    })?;
    let step_index = nonnegative_usize(record.get("stepIndex")).ok_or_else(|| {
        TranscriptJournalCorruptionError("transcript deferred cursor is invalid".into())
    })?;
    Ok(Some(DeferredTranscriptStep {
        turn_index,
        step_index,
    }))
}

fn nonnegative_usize(value: Option<&Value>) -> Option<usize> {
    value?.as_u64().and_then(|value| usize::try_from(value).ok())
}

fn lowercase_sha256(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Some(value.to_owned())
    } else {
        None
    }
}

pub fn parse_pending_checkpoint(
    raw: &str,
) -> Result<PendingTranscriptCheckpoint, TranscriptJournalCorruptionError> {
    let parsed: Value = serde_json::from_str(raw).map_err(|_| {
        TranscriptJournalCorruptionError("pending transcript checkpoint is not valid JSON".into())
    })?;
    let Some(record) = parsed.as_object() else {
        return Err(TranscriptJournalCorruptionError(
            "pending transcript checkpoint is not an object".into(),
        ));
    };
    if record.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(TranscriptJournalCorruptionError(
            "pending transcript checkpoint metadata is invalid".into(),
        ));
    }
    let previous_checkpoint_hash = lowercase_sha256(record.get("previousCheckpointHash"))
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let checkpoint_hash = lowercase_sha256(record.get("checkpointHash")).ok_or_else(|| {
        TranscriptJournalCorruptionError(
            "pending transcript checkpoint metadata is invalid".into(),
        )
    })?;
    let append_offset = record
        .get("appendOffset")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let file_device = record
        .get("fileDevice")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let file_inode = record
        .get("fileInode")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;

    let lines = record
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?
        .iter()
        .map(|line| {
            let line = line.as_str().ok_or_else(|| {
                TranscriptJournalCorruptionError("pending transcript line is invalid".into())
            })?;
            let value: Value = serde_json::from_str(line).map_err(|_| {
                TranscriptJournalCorruptionError("pending transcript line is invalid".into())
            })?;
            let valid_envelope = value
                .as_object()
                .is_some_and(|record| record.contains_key("role") && record.contains_key("message"));
            if !valid_envelope {
                return Err(TranscriptJournalCorruptionError(
                    "pending transcript line does not use the legacy message envelope".into(),
                ));
            }
            Ok(line.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let cursor = record
        .get("cursor")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            TranscriptJournalCorruptionError(
                "pending transcript checkpoint metadata is invalid".into(),
            )
        })?;
    let turn_count = nonnegative_usize(cursor.get("turnCount")).ok_or_else(|| {
        TranscriptJournalCorruptionError("pending transcript checkpoint cursor is invalid".into())
    })?;
    let deferred_step = match cursor.get("deferredStep") {
        Some(value) => parse_deferred_step(value)?,
        None => None,
    };

    Ok(PendingTranscriptCheckpoint {
        version: 1,
        previous_checkpoint_hash,
        checkpoint_hash,
        append_offset,
        file_device,
        file_inode,
        lines,
        turn_count,
        deferred_step,
    })
}

pub fn pending_checkpoint_json(pending: &PendingTranscriptCheckpoint) -> String {
    let cursor = match pending.deferred_step {
        Some(step) => json!({
            "turnCount": pending.turn_count,
            "deferredStep": {
                "turnIndex": step.turn_index,
                "stepIndex": step.step_index,
            }
        }),
        None => json!({"turnCount": pending.turn_count}),
    };
    json!({
        "version": 1,
        "previousCheckpointHash": pending.previous_checkpoint_hash,
        "checkpointHash": pending.checkpoint_hash,
        "appendOffset": pending.append_offset,
        "fileDevice": pending.file_device,
        "fileInode": pending.file_inode,
        "lines": pending.lines,
        "cursor": cursor,
    })
    .to_string()
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
        written += count;
    }
    Ok(position + written as u64)
}

pub fn format_text_line(role: &str, text: &str) -> Option<String> {
    if text.is_empty() || !matches!(role, "user" | "assistant") {
        return None;
    }
    Some(
        json!({
            "role": role,
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string(),
    )
}

pub fn format_tool_line(role: &str, name: &str, payload: Value) -> Option<String> {
    let content = match role {
        "assistant" => json!([{"type": "tool_use", "name": name, "input": payload}]),
        "tool" => json!([{"type": "tool_result", "name": name, "result": payload}]),
        _ => return None,
    };
    Some(json!({"role": role, "message": {"content": content}}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn checkpoint_identity_uses_turn_count_and_last_two_turns() {
        let one = TranscriptCheckpoint {
            turns: vec![vec![1, 2, 3]],
        };
        let changed_older = TranscriptCheckpoint {
            turns: vec![vec![9], vec![4], vec![5]],
        };
        let same_tail = TranscriptCheckpoint {
            turns: vec![vec![8], vec![4], vec![5]],
        };
        assert_ne!(checkpoint_identity(&TranscriptCheckpoint::default()), checkpoint_identity(&one));
        assert_eq!(checkpoint_identity(&changed_older), checkpoint_identity(&same_tail));
    }

    #[test]
    fn pending_checkpoint_round_trips_and_validates_legacy_envelopes() {
        let pending = PendingTranscriptCheckpoint {
            version: 1,
            previous_checkpoint_hash: "a".repeat(64),
            checkpoint_hash: "b".repeat(64),
            append_offset: 7,
            file_device: "1".into(),
            file_inode: "2".into(),
            lines: vec![json!({"role":"user","message":{"content":[]}}).to_string()],
            turn_count: 3,
            deferred_step: Some(DeferredTranscriptStep {
                turn_index: 2,
                step_index: 4,
            }),
        };
        let parsed = parse_pending_checkpoint(&pending_checkpoint_json(&pending)).unwrap();
        assert_eq!(parsed, pending);

        let invalid = pending_checkpoint_json(&PendingTranscriptCheckpoint {
            lines: vec![json!({"role":"user"}).to_string()],
            ..pending
        });
        assert!(parse_pending_checkpoint(&invalid).is_err());
    }

    #[test]
    fn formatting_matches_legacy_message_envelope() {
        let text = format_text_line("user", "hello").unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["message"]["content"][0]["type"], "text");

        let tool = format_tool_line("assistant", "search", json!({"q":"x"})).unwrap();
        let value: Value = serde_json::from_str(&tool).unwrap();
        assert_eq!(value["message"]["content"][0]["type"], "tool_use");
    }

    #[test]
    fn write_all_at_advances_from_requested_offset() {
        let mut cursor = Cursor::new(vec![0, 0]);
        assert_eq!(write_all_at(&mut cursor, b"abc", 2).unwrap(), 5);
        assert_eq!(cursor.into_inner(), b"\0\0abc");
    }
}
