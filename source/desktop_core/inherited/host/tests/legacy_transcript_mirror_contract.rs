use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::transcript_mirror::legacy_transcript_mirror::{
    LegacyFileTranscriptMirror, LegacyTranscriptBlobStore, LegacyTranscriptState,
    count_transcript_message_lines,
};

#[derive(Default)]
struct MemoryBlobs {
    values: HashMap<Vec<u8>, Vec<u8>>,
}

impl LegacyTranscriptBlobStore for MemoryBlobs {
    fn get_blob(&self, blob_id: &[u8]) -> Result<Option<Vec<u8>>, String> {
        Ok(self.values.get(blob_id).cloned())
    }
}

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-legacy-transcript-{label}-{}-{nanos}",
        std::process::id()
    ))
}

#[test]
fn full_mirror_formats_visible_jsonl_and_never_replaces_with_fewer_messages() {
    let root = temp_root("full");
    let mirror = LegacyFileTranscriptMirror::new(&root);
    let user_id = vec![1];
    let assistant_id = vec![2];
    let mut blobs = MemoryBlobs::default();
    blobs.values.insert(
        user_id.clone(),
        br#"{"role":"user","content":"hello\n<rules>hidden</rules>\nworld"}"#.to_vec(),
    );
    blobs.values.insert(
        assistant_id.clone(),
        br#"{"role":"assistant","content":"before <think>secret</think> after"}"#.to_vec(),
    );
    let state = LegacyTranscriptState {
        root_prompt_messages_json: vec![user_id.clone(), assistant_id.clone()],
        ..LegacyTranscriptState::default()
    };
    assert!(mirror.write_full("agent-a", &state, &blobs).expect("full write"));
    let path = mirror.jsonl_path_for("agent-a").expect("path");
    let content = fs::read_to_string(&path).expect("jsonl");
    assert_eq!(count_transcript_message_lines(&content), 2);
    assert!(!content.contains("hidden"));
    assert!(!content.contains("secret"));

    let shorter = LegacyTranscriptState {
        root_prompt_messages_json: vec![user_id],
        ..LegacyTranscriptState::default()
    };
    assert!(!mirror.write_full("agent-a", &shorter, &blobs).expect("stale full"));
    assert_eq!(
        count_transcript_message_lines(&fs::read_to_string(&path).expect("preserved")),
        2
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn incremental_mirror_appends_only_new_root_prompt_messages() {
    let root = temp_root("incremental");
    let mirror = LegacyFileTranscriptMirror::new(&root);
    let first = vec![1];
    let second = vec![2];
    let mut blobs = MemoryBlobs::default();
    blobs.values.insert(first.clone(), br#"{"role":"user","content":"one"}"#.to_vec());
    blobs.values.insert(second.clone(), br#"{"role":"assistant","content":"two"}"#.to_vec());

    let initial = LegacyTranscriptState {
        root_prompt_messages_json: vec![first.clone()],
        ..LegacyTranscriptState::default()
    };
    assert!(mirror.write_full("agent-b", &initial, &blobs).expect("initial"));

    let next = LegacyTranscriptState {
        root_prompt_messages_json: vec![first, second],
        ..LegacyTranscriptState::default()
    };
    assert_eq!(
        mirror.write_incremental("agent-b", &next, &blobs, 1).expect("incremental"),
        Some(2)
    );
    let content = fs::read_to_string(mirror.jsonl_path_for("agent-b").expect("path"))
        .expect("jsonl");
    assert_eq!(count_transcript_message_lines(&content), 2);
    assert!(content.contains("\"text\":\"one\""));
    assert!(content.contains("\"text\":\"two\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn binary_markers_and_tool_calls_use_observational_projection() {
    let root = temp_root("binary");
    let mirror = LegacyFileTranscriptMirror::new(&root);
    let id = vec![7];
    let mut blobs = MemoryBlobs::default();
    blobs.values.insert(
        id.clone(),
        br#"{"role":"assistant","content":[{"type":"text","text":"x"},{"type":"tool-call","toolName":"search","args":{"raw":{"__type":"Uint8Array","hex":"aabb"}}}]}"#.to_vec(),
    );
    let state = LegacyTranscriptState {
        root_prompt_messages_json: vec![id],
        ..LegacyTranscriptState::default()
    };
    assert!(mirror.write_full("agent-c", &state, &blobs).expect("write"));
    let content = fs::read_to_string(mirror.jsonl_path_for("agent-c").expect("path"))
        .expect("jsonl");
    assert!(content.contains("tool_use"));
    assert!(content.contains("[Binary data omitted from transcript: 2 bytes]"));
    let _ = fs::remove_dir_all(root);
}
