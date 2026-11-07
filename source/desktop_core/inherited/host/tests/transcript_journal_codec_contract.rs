use std::io::{Cursor, Seek, SeekFrom};

use mahayana_host_runtime::transcript_mirror::transcript_journal_codec::{
    DeferredTranscriptStep, TranscriptCheckpoint, checkpoint_identity, format_text_line,
    format_tool_line, parse_deferred_step, parse_pending_checkpoint, write_all_at,
};
use serde_json::json;

#[test]
fn checkpoint_identity_uses_only_length_and_last_two_turn_ids() {
    let one = TranscriptCheckpoint {
        turns: vec![vec![0xaa]],
    };
    let one_changed_prefix = TranscriptCheckpoint {
        turns: vec![vec![0xbb]],
    };
    assert_ne!(checkpoint_identity(&one), checkpoint_identity(&one_changed_prefix));

    let a = TranscriptCheckpoint {
        turns: vec![vec![1], vec![2], vec![3]],
    };
    let b = TranscriptCheckpoint {
        turns: vec![vec![9], vec![2], vec![3]],
    };
    assert_eq!(checkpoint_identity(&a), checkpoint_identity(&b));
}

#[test]
fn pending_checkpoint_validation_matches_frozen_legacy_envelope_contract() {
    let raw = json!({
        "version": 1,
        "previousCheckpointHash": "a".repeat(64),
        "checkpointHash": "b".repeat(64),
        "appendOffset": 17,
        "fileDevice": "2",
        "fileInode": "4",
        "lines": [
            json!({"role":"user","message":{"content":[]}}).to_string(),
            json!({"role":"assistant","message":{"content":[]}}).to_string()
        ],
        "cursor": {
            "turnCount": 3,
            "deferredStep": {"turnIndex":2,"stepIndex":1}
        }
    })
    .to_string();
    let parsed = parse_pending_checkpoint(&raw).expect("pending checkpoint");
    assert_eq!(parsed.append_offset, 17);
    assert_eq!(
        parsed.cursor.deferred_step,
        Some(DeferredTranscriptStep {
            turn_index: 2,
            step_index: 1,
        })
    );

    let invalid = json!({
        "version": 1,
        "previousCheckpointHash": "A".repeat(64),
        "checkpointHash": "b".repeat(64),
        "appendOffset": 0,
        "fileDevice": "2",
        "fileInode": "4",
        "lines": [],
        "cursor": {"turnCount":0}
    })
    .to_string();
    assert!(parse_pending_checkpoint(&invalid).is_err());
    assert!(parse_deferred_step(Some(&json!({"turnIndex": -1, "stepIndex": 0}))).is_err());
}

#[test]
fn jsonl_formatters_preserve_legacy_message_envelope() {
    assert_eq!(format_text_line("user", ""), None);
    let text: serde_json::Value =
        serde_json::from_str(&format_text_line("user", "hello").expect("line")).unwrap();
    assert_eq!(text["role"], "user");
    assert_eq!(text["message"]["content"][0]["type"], "text");
    assert_eq!(text["message"]["content"][0]["text"], "hello");

    let tool: serde_json::Value =
        serde_json::from_str(&format_tool_line("assistant", "search", json!({"q":"x"}))).unwrap();
    assert_eq!(tool["message"]["content"][0]["type"], "tool_use");
    assert_eq!(tool["message"]["content"][0]["input"]["q"], "x");
}

#[test]
fn write_all_at_writes_from_requested_offset_and_reports_final_position() {
    let mut cursor = Cursor::new(vec![0u8; 3]);
    cursor.seek(SeekFrom::Start(0)).unwrap();
    let end = write_all_at(&mut cursor, b"abc", 3).expect("write");
    assert_eq!(end, 6);
    assert_eq!(cursor.into_inner(), b"\0\0\0abc");
}
